use crate::{config::Settings, network::protocol::*, FileshareError, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::time::sleep;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

// Phase 1 constants and limits
const MAX_FILE_SIZE_PHASE1: u64 = 100 * 1024 * 1024; // 100MB limit for Phase 1
const MAX_RETRY_ATTEMPTS: u32 = 3;
const RETRY_BASE_DELAY_MS: u64 = 1000; // 1 second base delay
const MAX_RETRY_DELAY_MS: u64 = 8000; // 8 seconds max delay
const TRANSFER_TIMEOUT_SECONDS: u64 = 300; // 5 minutes per transfer
const CHUNK_TIMEOUT_SECONDS: u64 = 30; // 30 seconds per chunk

// Use simple message sender instead of DirectPeerSender
pub type MessageSender = mpsc::UnboundedSender<(Uuid, Message)>;

// Retry state management
#[derive(Debug, Clone)]
pub struct RetryState {
    pub attempt_count: u32,
    pub last_error: Option<String>,
    pub next_retry_at: Option<Instant>,
    pub total_retry_delay: u64,
}

impl Default for RetryState {
    fn default() -> Self {
        Self {
            attempt_count: 0,
            last_error: None,
            next_retry_at: None,
            total_retry_delay: 0,
        }
    }
}

#[derive(Debug)]
pub struct FileTransfer {
    pub id: Uuid,
    pub peer_id: Uuid,
    pub metadata: FileMetadata,
    pub file_path: PathBuf,
    pub direction: TransferDirection,
    pub status: TransferStatus,
    pub bytes_transferred: u64,
    pub chunks_received: Vec<bool>,
    pub file_handle: Option<std::fs::File>,
    pub received_data: Vec<u8>,
    pub created_at: Instant,
    pub retry_state: RetryState, // Enhanced retry tracking
    pub last_activity: Instant,  // Track last activity for timeouts
}

#[derive(Debug, Clone)]
pub enum TransferDirection {
    Outgoing,
    Incoming,
}

#[derive(Debug, Clone)]
pub enum TransferStatus {
    Pending,
    Active,
    Paused,
    Completed,
    Error(String),
    Cancelled,
}

pub struct FileTransferManager {
    settings: Arc<Settings>,
    pub active_transfers: HashMap<Uuid, FileTransfer>,
    message_sender: Option<MessageSender>,
}

impl FileTransferManager {
    pub async fn new(settings: Arc<Settings>) -> Result<Self> {
        Ok(Self {
            settings,
            active_transfers: HashMap::new(),
            message_sender: None,
        })
    }

    // Set the message sender (replaces DirectPeerSender)
    pub fn set_message_sender(&mut self, sender: MessageSender) {
        self.message_sender = Some(sender);
    }

    pub async fn mark_outgoing_transfer_completed(&mut self, transfer_id: Uuid) -> Result<()> {
        // First scope: Update the transfer status
        {
            if let Some(transfer) = self.active_transfers.get_mut(&transfer_id) {
                if matches!(transfer.direction, TransferDirection::Outgoing) {
                    info!(
                        "✅ SENDER: Marking outgoing transfer {} as completed",
                        transfer_id
                    );
                    transfer.status = TransferStatus::Completed;
                    transfer.last_activity = Instant::now();
                } else {
                    warn!(
                        "⚠️ Attempted to mark non-outgoing transfer {} as completed",
                        transfer_id
                    );
                    return Ok(());
                }
            } else {
                warn!(
                    "⚠️ Transfer {} not found when marking as completed",
                    transfer_id
                );
                return Ok(());
            }
        } // Mutable borrow ends here

        // Second scope: Show notification with immutable borrow
        {
            if let Some(transfer) = self.active_transfers.get(&transfer_id) {
                if let Err(e) = self.show_transfer_notification(transfer).await {
                    warn!("Failed to show notification: {}", e);
                }
            }
        }

        Ok(())
    }

    // Phase 1: Validate file size before transfer
    pub fn validate_file_size(&self, file_path: &PathBuf) -> Result<()> {
        let metadata = std::fs::metadata(file_path)
            .map_err(|e| FileshareError::FileOperation(format!("Cannot access file: {}", e)))?;

        let file_size = metadata.len();

        if file_size == 0 {
            return Err(FileshareError::FileOperation(
                "Cannot transfer empty files".to_string(),
            ));
        }

        if file_size > MAX_FILE_SIZE_PHASE1 {
            return Err(FileshareError::FileOperation(format!(
                "File size ({} MB) exceeds maximum allowed size ({} MB) for Phase 1",
                file_size / (1024 * 1024),
                MAX_FILE_SIZE_PHASE1 / (1024 * 1024)
            )));
        }

        info!("✅ File size validation passed: {} bytes", file_size);
        Ok(())
    }

    // Enhanced send_file with validation and retry
    pub async fn send_file_with_validation(
        &mut self,
        peer_id: Uuid,
        file_path: PathBuf,
    ) -> Result<()> {
        info!(
            "🚀 ENHANCED_SEND: Starting validated file transfer to {}: {:?}",
            peer_id, file_path
        );

        // Phase 1: Validate file before transfer
        self.validate_file_size(&file_path)?;

        // Check file exists and is readable
        if !file_path.exists() {
            return Err(FileshareError::FileOperation(
                "File does not exist".to_string(),
            ));
        }

        if !file_path.is_file() {
            return Err(FileshareError::FileOperation(
                "Path is not a file".to_string(),
            ));
        }

        // Attempt transfer with retry logic
        self.send_file_with_retry(peer_id, file_path, 0).await
    }

    // Fixed version - using loop instead of recursion
    async fn send_file_with_retry(
        &mut self,
        peer_id: Uuid,
        file_path: PathBuf,
        mut attempt: u32,
    ) -> Result<()> {
        loop {
            match self
                .send_file_with_target_dir(peer_id, file_path.clone(), None)
                .await
            {
                Ok(()) => {
                    info!("✅ File transfer successful on attempt {}", attempt + 1);
                    return Ok(());
                }
                Err(e) => {
                    error!("❌ File transfer attempt {} failed: {}", attempt + 1, e);

                    if attempt < MAX_RETRY_ATTEMPTS {
                        let delay = self.calculate_retry_delay(attempt);
                        warn!(
                            "🔄 Retrying in {} seconds (attempt {}/{})",
                            delay.as_secs(),
                            attempt + 2,
                            MAX_RETRY_ATTEMPTS + 1
                        );

                        sleep(delay).await;
                        attempt += 1; // Increment for next iteration
                                      // Continue the loop for the next attempt
                    } else {
                        error!(
                            "💥 File transfer failed after {} attempts",
                            MAX_RETRY_ATTEMPTS + 1
                        );
                        return Err(FileshareError::Transfer(format!(
                            "Transfer failed after {} attempts: {}",
                            MAX_RETRY_ATTEMPTS + 1,
                            e
                        )));
                    }
                }
            }
        }
    }

    // Calculate exponential backoff delay
    fn calculate_retry_delay(&self, attempt: u32) -> Duration {
        let delay_ms = std::cmp::min(
            RETRY_BASE_DELAY_MS * (2_u64.pow(attempt)),
            MAX_RETRY_DELAY_MS,
        );
        Duration::from_millis(delay_ms)
    }

    // Enhanced monitor that excludes completed/successful transfers
    pub async fn monitor_transfer_health(&mut self) -> Result<()> {
        let now = Instant::now();
        let mut timed_out_transfers = Vec::new();
        let mut stale_transfers = Vec::new();

        for (transfer_id, transfer) in &self.active_transfers {
            // CRITICAL: Skip monitoring for completed or successful transfers
            match &transfer.status {
                TransferStatus::Completed => {
                    info!(
                        "✅ Skipping health check for completed transfer {}",
                        transfer_id
                    );
                    continue;
                }
                TransferStatus::Error(_) => {
                    info!(
                        "⚠️ Skipping health check for errored transfer {}",
                        transfer_id
                    );
                    continue;
                }
                TransferStatus::Cancelled => {
                    info!(
                        "🚫 Skipping health check for cancelled transfer {}",
                        transfer_id
                    );
                    continue;
                }
                _ => {} // Continue monitoring active/pending transfers
            }

            // Check for overall transfer timeout
            if now.duration_since(transfer.created_at).as_secs() > TRANSFER_TIMEOUT_SECONDS {
                error!(
                    "⏰ Transfer {} timed out after {} seconds",
                    transfer_id, TRANSFER_TIMEOUT_SECONDS
                );
                timed_out_transfers.push(*transfer_id);
                continue;
            }

            // Check for inactive transfers (no activity for chunk timeout)
            if now.duration_since(transfer.last_activity).as_secs() > CHUNK_TIMEOUT_SECONDS {
                match transfer.status {
                    TransferStatus::Active | TransferStatus::Pending => {
                        warn!(
                            "⚠️ Transfer {} inactive for {} seconds, marking for recovery",
                            transfer_id, CHUNK_TIMEOUT_SECONDS
                        );
                        stale_transfers.push(*transfer_id);
                    }
                    _ => {} // Ignore non-active transfers
                }
            }
        }

        // Handle timed out transfers
        for transfer_id in timed_out_transfers {
            self.handle_transfer_timeout(transfer_id).await?;
        }

        // Attempt recovery for stale transfers
        for transfer_id in stale_transfers {
            self.attempt_transfer_recovery(transfer_id).await?;
        }

        Ok(())
    }

    // Handle transfer timeout
    async fn handle_transfer_timeout(&mut self, transfer_id: Uuid) -> Result<()> {
        if let Some(transfer) = self.active_transfers.get_mut(&transfer_id) {
            transfer.status = TransferStatus::Error("Transfer timed out".to_string());

            // Notify UI about timeout
            if let Some(ref sender) = self.message_sender {
                let error_msg = Message::new(MessageType::TransferError {
                    transfer_id,
                    error: "Transfer timed out after 5 minutes".to_string(),
                });
                let _ = sender.send((transfer.peer_id, error_msg));
            }

            // Clean up resources
            self.cleanup_transfer_resources(transfer_id).await?;
        }
        Ok(())
    }

    // Enhanced recovery that checks completion status
    async fn attempt_transfer_recovery(&mut self, transfer_id: Uuid) -> Result<()> {
        // First, verify the transfer still needs recovery
        let recovery_data = if let Some(transfer) = self.active_transfers.get_mut(&transfer_id) {
            // CRITICAL: Don't recover completed/successful transfers
            match &transfer.status {
                TransferStatus::Completed => {
                    info!(
                        "✅ Skipping recovery for completed transfer {}",
                        transfer_id
                    );
                    return Ok(());
                }
                TransferStatus::Error(_) => {
                    info!("⚠️ Skipping recovery for errored transfer {}", transfer_id);
                    return Ok(());
                }
                TransferStatus::Cancelled => {
                    info!(
                        "🚫 Skipping recovery for cancelled transfer {}",
                        transfer_id
                    );
                    return Ok(());
                }
                _ => {} // Continue with recovery for active/pending transfers
            }

            // Only attempt recovery for outgoing transfers that are genuinely stale
            if matches!(transfer.direction, TransferDirection::Outgoing)
                && matches!(
                    transfer.status,
                    TransferStatus::Active | TransferStatus::Pending
                )
            {
                info!("🔄 Attempting recovery for stale transfer {}", transfer_id);

                // Update retry state
                transfer.retry_state.attempt_count += 1;
                transfer.last_activity = Instant::now();

                if transfer.retry_state.attempt_count <= MAX_RETRY_ATTEMPTS {
                    // Extract the data we need for recovery
                    let file_path = transfer.file_path.clone();
                    let peer_id = transfer.peer_id;
                    let attempt_count = transfer.retry_state.attempt_count;

                    info!(
                        "🔄 Recovery attempt {} for transfer {}",
                        attempt_count, transfer_id
                    );

                    Some((file_path, peer_id, attempt_count))
                } else {
                    error!(
                        "💥 Transfer {} failed after {} recovery attempts",
                        transfer_id, MAX_RETRY_ATTEMPTS
                    );
                    transfer.status = TransferStatus::Error(
                        "Transfer failed after multiple recovery attempts".to_string(),
                    );
                    None
                }
            } else {
                info!(
                    "ℹ️ Transfer {} doesn't need recovery (not outgoing or not active)",
                    transfer_id
                );
                None // Not an outgoing transfer or not in recoverable state
            }
        } else {
            warn!(
                "⚠️ Transfer {} not found during recovery attempt",
                transfer_id
            );
            None // Transfer not found
        };

        // Now handle recovery outside of the borrow
        if let Some((file_path, peer_id, attempt_count)) = recovery_data {
            // Remove the old transfer and start fresh
            info!("🔄 Removing old transfer {} for fresh retry", transfer_id);
            self.active_transfers.remove(&transfer_id);

            // Retry after a delay
            let delay = self.calculate_retry_delay(attempt_count - 1);
            sleep(delay).await;

            // Now we can safely call send_file_with_target_dir
            info!("🚀 Starting fresh transfer after recovery delay");
            self.send_file_with_target_dir(peer_id, file_path, None)
                .await?;
        }

        Ok(())
    }

    // Clean up transfer resources
    async fn cleanup_transfer_resources(&mut self, transfer_id: Uuid) -> Result<()> {
        if let Some(transfer) = self.active_transfers.remove(&transfer_id) {
            info!("🧹 Cleaning up resources for transfer {}", transfer_id);

            // Close file handles if any
            if let Some(_file_handle) = transfer.file_handle {
                // File handle will be automatically closed when dropped
            }

            // For incomplete incoming transfers, clean up temporary files
            if matches!(transfer.direction, TransferDirection::Incoming)
                && !matches!(transfer.status, TransferStatus::Completed)
            {
                if transfer.file_path.exists() {
                    if let Err(e) = std::fs::remove_file(&transfer.file_path) {
                        warn!(
                            "Failed to clean up incomplete file {:?}: {}",
                            transfer.file_path, e
                        );
                    } else {
                        info!("🗑️ Cleaned up incomplete file: {:?}", transfer.file_path);
                    }
                }
            }
        }
        Ok(())
    }

    // Enhanced cleanup that protects completed transfers
    pub fn cleanup_stale_transfers_enhanced(&mut self) {
        let now = Instant::now();
        let timeout = Duration::from_secs(TRANSFER_TIMEOUT_SECONDS);
        let completion_grace_period = Duration::from_secs(30); // 30 seconds grace for completed transfers

        let initial_count = self.active_transfers.len();

        self.active_transfers.retain(|transfer_id, transfer| {
            // CRITICAL: Give completed transfers a grace period before cleanup
            match &transfer.status {
                TransferStatus::Completed => {
                    if now.duration_since(transfer.last_activity) > completion_grace_period {
                        info!("🧹 Cleaning up completed transfer: {}", transfer_id);
                        return false;
                    } else {
                        info!("✅ Protecting recently completed transfer: {}", transfer_id);
                        return true; // Keep it for grace period
                    }
                }
                TransferStatus::Error(_) => {
                    if now.duration_since(transfer.last_activity) > Duration::from_secs(60) {
                        info!("🧹 Cleaning up errored transfer: {}", transfer_id);
                        return false;
                    }
                    return true;
                }
                TransferStatus::Cancelled => {
                    if now.duration_since(transfer.last_activity) > Duration::from_secs(10) {
                        info!("🧹 Cleaning up cancelled transfer: {}", transfer_id);
                        return false;
                    }
                    return true;
                }
                _ => {} // Continue with normal cleanup logic for active transfers
            }

            // Remove transfers that have been running too long
            if now.duration_since(transfer.created_at) > timeout {
                warn!("🧹 Removing timed out transfer: {}", transfer_id);
                return false;
            }

            // Remove transfers that have been inactive for too long (only active ones)
            if matches!(
                transfer.status,
                TransferStatus::Active | TransferStatus::Pending
            ) && now.duration_since(transfer.last_activity)
                > Duration::from_secs(CHUNK_TIMEOUT_SECONDS)
            {
                warn!("🧹 Removing inactive transfer: {}", transfer_id);
                return false;
            }

            true
        });

        let removed_count = initial_count - self.active_transfers.len();
        if removed_count > 0 {
            info!("🧹 Enhanced cleanup removed {} transfers", removed_count);
        }
    }

    pub async fn send_file_with_target_dir(
        &mut self,
        peer_id: Uuid,
        file_path: PathBuf,
        target_dir: Option<String>,
    ) -> Result<()> {
        info!(
            "🚀 SEND_FILE: Starting file transfer to {}: {:?} (target_dir: {:?})",
            peer_id, file_path, target_dir
        );

        // Clean up stale transfers first
        self.cleanup_stale_transfers_enhanced();

        if !file_path.exists() {
            return Err(FileshareError::FileOperation(
                "File does not exist".to_string(),
            ));
        }

        if !file_path.is_file() {
            return Err(FileshareError::FileOperation(
                "Path is not a file".to_string(),
            ));
        }

        // Use the same chunk size as configured for this transfer manager
        let chunk_size = self.settings.transfer.chunk_size;
        let metadata = FileMetadata::from_path_with_chunk_size(&file_path, chunk_size)?
            .with_target_dir(target_dir);
        let transfer_id = Uuid::new_v4();

        info!(
            "🚀 SEND_FILE: Created transfer {} for peer {}, file size: {} bytes, chunk size: {} bytes, total chunks: {}",
            transfer_id, peer_id, metadata.size, metadata.chunk_size, metadata.total_chunks
        );

        // Check for existing transfer with same file to same peer
        for (existing_id, existing_transfer) in &self.active_transfers {
            if existing_transfer.peer_id == peer_id
                && existing_transfer.file_path == file_path
                && matches!(existing_transfer.direction, TransferDirection::Outgoing)
                && !matches!(
                    existing_transfer.status,
                    TransferStatus::Completed
                        | TransferStatus::Error(_)
                        | TransferStatus::Cancelled
                )
            {
                warn!(
                    "🚫 Transfer already in progress for this file to this peer: {}",
                    existing_id
                );
                return Err(FileshareError::Transfer(
                    "Transfer already in progress for this file".to_string(),
                ));
            }
        }

        // Create and store the transfer BEFORE sending the offer
        let transfer = FileTransfer {
            id: transfer_id,
            peer_id,
            metadata: metadata.clone(),
            file_path: file_path.clone(),
            direction: TransferDirection::Outgoing,
            status: TransferStatus::Pending,
            bytes_transferred: 0,
            chunks_received: Vec::new(),
            file_handle: None,
            received_data: Vec::new(),
            created_at: Instant::now(),
            retry_state: RetryState::default(),
            last_activity: Instant::now(),
        };

        // Store the transfer first
        self.active_transfers.insert(transfer_id, transfer);
        info!(
            "🚀 SEND_FILE: Registered outgoing transfer {} for peer {}",
            transfer_id, peer_id
        );

        // Send FileOffer through normal message channel
        if let Some(ref sender) = self.message_sender {
            let file_offer = Message::new(MessageType::FileOffer {
                transfer_id,
                metadata: metadata.clone(),
            });

            info!(
                "🚀 SEND_FILE: Sending FileOffer {} to message channel for peer {}",
                transfer_id, peer_id
            );

            if let Err(e) = sender.send((peer_id, file_offer)) {
                error!("Failed to send file offer: {}", e);
                self.active_transfers.remove(&transfer_id);
                return Err(FileshareError::Transfer(format!(
                    "Failed to send file offer: {}",
                    e
                )));
            }

            info!(
                "🚀 SEND_FILE: FileOffer {} sent to message channel for peer {}",
                transfer_id, peer_id
            );
        } else {
            self.active_transfers.remove(&transfer_id);
            return Err(FileshareError::Transfer(
                "Message sender not configured".to_string(),
            ));
        }

        Ok(())
    }

    pub async fn send_file(&mut self, peer_id: Uuid, file_path: PathBuf) -> Result<()> {
        self.send_file_with_target_dir(peer_id, file_path, None)
            .await
    }

    pub async fn handle_file_offer_response(
        &mut self,
        peer_id: Uuid,
        transfer_id: Uuid,
        accepted: bool,
        reason: Option<String>,
    ) -> Result<()> {
        info!(
            "🚀 RECEIVED FileOfferResponse for transfer {} from peer {}: accepted={}",
            transfer_id, peer_id, accepted
        );

        let transfer = self.active_transfers.get(&transfer_id);
        if transfer.is_none() {
            warn!(
                "Received FileOfferResponse for unknown transfer {}",
                transfer_id
            );
            return Ok(());
        }

        let is_outgoing = matches!(transfer.unwrap().direction, TransferDirection::Outgoing);

        if !is_outgoing {
            info!(
                "Ignoring FileOfferResponse for incoming transfer {}",
                transfer_id
            );
            return Ok(());
        }

        if !accepted {
            info!(
                "File offer {} was rejected by peer {}: {:?}",
                transfer_id, peer_id, reason
            );
            if let Some(transfer) = self.active_transfers.get_mut(&transfer_id) {
                transfer.status = TransferStatus::Cancelled;
            }
            return Ok(());
        }

        info!(
            "✅ File offer {} accepted by peer {}, starting file transfer",
            transfer_id, peer_id
        );

        self.start_file_transfer(transfer_id).await?;
        Ok(())
    }

    async fn start_file_transfer(&mut self, transfer_id: Uuid) -> Result<()> {
        let (file_path, peer_id, chunk_size) = {
            let transfer = self
                .active_transfers
                .get_mut(&transfer_id)
                .ok_or_else(|| FileshareError::Transfer("Transfer not found".to_string()))?;

            transfer.status = TransferStatus::Active;
            transfer.last_activity = Instant::now();

            // Use the chunk size from the metadata (what was agreed upon)
            let chunk_size = transfer.metadata.chunk_size;

            (transfer.file_path.clone(), transfer.peer_id, chunk_size)
        };

        let message_sender = self
            .message_sender
            .clone()
            .ok_or_else(|| FileshareError::Transfer("Message sender not configured".to_string()))?;

        tokio::spawn(async move {
            if let Err(e) =
                Self::send_file_chunks(message_sender, peer_id, transfer_id, file_path, chunk_size)
                    .await
            {
                error!("Failed to send file chunks: {}", e);
            }
        });

        Ok(())
    }

    async fn send_file_chunks(
        message_sender: MessageSender,
        peer_id: Uuid,
        transfer_id: Uuid,
        file_path: PathBuf,
        chunk_size: usize,
    ) -> Result<()> {
        use sha2::{Digest, Sha256};

        info!(
            "🚀 CHUNK_SENDER: Starting to send file chunks for transfer {} to peer {}",
            transfer_id, peer_id
        );
        info!("📄 Reading file: {:?}", file_path);

        // Read the entire file into memory first, then send chunks
        let file_data = std::fs::read(&file_path).map_err(|e| {
            error!("Failed to read file {:?}: {}", file_path, e);
            FileshareError::FileOperation(format!("Failed to read file: {}", e))
        })?;

        info!(
            "📊 File size: {} bytes, chunk size: {} bytes",
            file_data.len(),
            chunk_size
        );

        // Calculate expected number of chunks
        let expected_chunks = (file_data.len() + chunk_size - 1) / chunk_size;
        info!("📦 Expected chunks: {}", expected_chunks);

        let mut hasher = Sha256::new();
        hasher.update(&file_data);

        let mut chunk_index = 0u64;
        let mut bytes_sent = 0;

        // Send file in chunks
        while bytes_sent < file_data.len() {
            let chunk_start = bytes_sent;
            let chunk_end = std::cmp::min(bytes_sent + chunk_size, file_data.len());
            let chunk_data = file_data[chunk_start..chunk_end].to_vec();
            let is_last = chunk_end >= file_data.len();

            info!(
                "📤 CHUNK_SENDER: Sending chunk {} for transfer {}: bytes {}-{} ({} bytes, is_last: {})",
                chunk_index, transfer_id, chunk_start, chunk_end - 1, chunk_data.len(), is_last
            );

            let chunk = TransferChunk {
                index: chunk_index,
                data: chunk_data,
                is_last,
            };

            let message = Message::new(MessageType::FileChunk { transfer_id, chunk });

            // Send through normal message channel
            if let Err(e) = message_sender.send((peer_id, message)) {
                error!(
                    "❌ CHUNK_SENDER: Failed to send chunk {}: {}",
                    chunk_index, e
                );
                let error_msg = Message::new(MessageType::TransferError {
                    transfer_id,
                    error: format!("Failed to send chunk: {}", e),
                });
                let _ = message_sender.send((peer_id, error_msg));
                return Err(FileshareError::Transfer(format!(
                    "Failed to send chunk: {}",
                    e
                )));
            }

            bytes_sent = chunk_end;
            chunk_index += 1;

            // Small delay to prevent overwhelming the network
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        // Send completion message through normal channel
        let checksum = format!("{:x}", hasher.finalize());
        info!(
            "✅ CHUNK_SENDER: File transfer complete for {}. Total bytes sent: {}, Total chunks: {}, Checksum: {}",
            transfer_id, bytes_sent, chunk_index, checksum
        );

        let complete_msg = Message::new(MessageType::TransferComplete {
            transfer_id,
            checksum,
        });

        if let Err(e) = message_sender.send((peer_id, complete_msg)) {
            error!("❌ CHUNK_SENDER: Failed to send transfer complete: {}", e);
        } else {
            info!(
                "✅ CHUNK_SENDER: File transfer {} completed successfully",
                transfer_id
            );
        }

        Ok(())
    }

    pub async fn handle_file_offer(
        &mut self,
        peer_id: Uuid,
        transfer_id: Uuid,
        metadata: FileMetadata,
    ) -> Result<()> {
        info!(
            "📥 RECEIVER: Received file offer from {}: {} ({} bytes, chunk_size: {}, total_chunks: {})",
            peer_id, metadata.name, metadata.size, metadata.chunk_size, metadata.total_chunks
        );

        // Clean up stale transfers first
        self.cleanup_stale_transfers_enhanced();

        // Check if transfer already exists
        if self.active_transfers.contains_key(&transfer_id) {
            warn!(
                "⚠️ RECEIVER: Transfer {} already exists, ignoring duplicate offer",
                transfer_id
            );
            return Ok(());
        }

        // Use target directory from metadata
        let save_path = self.get_save_path(&metadata.name, metadata.target_dir.as_deref())?;

        // Initialize received_data buffer with the expected size
        let received_data = vec![0u8; metadata.size as usize];

        // Use the chunk count from metadata (sender's calculation)
        let expected_chunks = metadata.total_chunks as usize;

        info!(
            "📦 RECEIVER: Transfer {} expects {} chunks of max {} bytes each (from sender metadata)",
            transfer_id, expected_chunks, metadata.chunk_size
        );

        let transfer = FileTransfer {
            id: transfer_id,
            peer_id,
            metadata: metadata.clone(),
            file_path: save_path.clone(),
            direction: TransferDirection::Incoming,
            status: TransferStatus::Pending,
            bytes_transferred: 0,
            chunks_received: vec![false; expected_chunks],
            file_handle: None,
            received_data,
            created_at: Instant::now(),
            retry_state: RetryState::default(),
            last_activity: Instant::now(),
        };

        self.active_transfers.insert(transfer_id, transfer);
        self.accept_file_transfer(transfer_id).await?;

        info!(
            "✅ RECEIVER: File offer accepted for transfer {}",
            transfer_id
        );
        Ok(())
    }

    pub fn create_file_offer_response(
        &self,
        transfer_id: Uuid,
        accepted: bool,
        reason: Option<String>,
    ) -> Message {
        Message::new(MessageType::FileOfferResponse {
            transfer_id,
            accepted,
            reason,
        })
    }

    async fn accept_file_transfer(&mut self, transfer_id: Uuid) -> Result<()> {
        let transfer = self
            .active_transfers
            .get_mut(&transfer_id)
            .ok_or_else(|| FileshareError::Transfer("Transfer not found".to_string()))?;

        transfer.status = TransferStatus::Active;
        transfer.last_activity = Instant::now();

        info!(
            "✅ RECEIVER: Accepted file transfer {} - will save to {:?}",
            transfer_id, transfer.file_path
        );

        Ok(())
    }

    pub async fn handle_file_chunk(
        &mut self,
        peer_id: Uuid,
        transfer_id: Uuid,
        chunk: TransferChunk,
    ) -> Result<()> {
        info!(
            "📥 RECEIVER: Received chunk {} for transfer {} ({} bytes, is_last: {})",
            chunk.index,
            transfer_id,
            chunk.data.len(),
            chunk.is_last
        );

        // Check if transfer exists and is incoming
        let transfer = self.active_transfers.get(&transfer_id);
        if let Some(transfer) = transfer {
            // Only process chunks for INCOMING transfers
            if !matches!(transfer.direction, TransferDirection::Incoming) {
                warn!(
                    "⚠️ RECEIVER: Ignoring chunk {} for outgoing transfer {} - chunks should only be processed for incoming transfers",
                    chunk.index, transfer_id
                );
                return Ok(());
            }
        } else {
            warn!(
                "⚠️ RECEIVER: Received chunk for unknown transfer {}",
                transfer_id
            );
            return Ok(());
        }

        let is_complete = {
            let transfer = self.active_transfers.get_mut(&transfer_id).unwrap();

            if transfer.peer_id != peer_id {
                return Err(FileshareError::Transfer(
                    "Chunk from wrong peer".to_string(),
                ));
            }

            if !matches!(transfer.status, TransferStatus::Active) {
                return Err(FileshareError::Transfer("Transfer not active".to_string()));
            }

            // Update last activity timestamp
            transfer.last_activity = Instant::now();

            // Use the chunk size from the agreed metadata (not local settings)
            let chunk_size = transfer.metadata.chunk_size as u64;
            let expected_offset = chunk.index * chunk_size;
            let actual_end_offset = expected_offset + chunk.data.len() as u64;
            let expected_file_size = transfer.received_data.len() as u64;

            // Enhanced validation with better error messages
            if chunk.index >= transfer.chunks_received.len() as u64 {
                error!(
                    "❌ RECEIVER: Chunk index {} out of bounds for transfer {}",
                    chunk.index, transfer_id
                );
                error!(
                    "   Expected chunks: {} (from metadata: {})",
                    transfer.chunks_received.len(),
                    transfer.metadata.total_chunks
                );
                error!(
                    "   File size: {} bytes, Chunk size: {} bytes",
                    expected_file_size, chunk_size
                );
                return Err(FileshareError::Transfer(format!(
                    "Chunk index {} out of bounds (expected max {})",
                    chunk.index,
                    transfer.chunks_received.len() - 1
                )));
            }

            if actual_end_offset > expected_file_size {
                error!(
                    "❌ RECEIVER: Chunk {} extends beyond expected file size for transfer {}",
                    chunk.index, transfer_id
                );
                error!("   Expected file size: {} bytes", expected_file_size);
                error!(
                    "   Chunk offset: {} + {} = {} bytes",
                    expected_offset,
                    chunk.data.len(),
                    actual_end_offset
                );
                error!("   Agreed chunk size: {} bytes", chunk_size);
                return Err(FileshareError::Transfer(format!(
                    "Chunk {} too large: offset {} + size {} = {} exceeds file size {}",
                    chunk.index,
                    expected_offset,
                    chunk.data.len(),
                    actual_end_offset,
                    expected_file_size
                )));
            }

            // Copy chunk data directly into the buffer
            let start_idx = expected_offset as usize;
            let end_idx = actual_end_offset as usize;
            transfer.received_data[start_idx..end_idx].copy_from_slice(&chunk.data);

            info!(
                "✅ RECEIVER: Copied chunk {} to buffer at offset {} (length: {})",
                chunk.index,
                expected_offset,
                chunk.data.len()
            );

            // Mark chunk as received
            transfer.chunks_received[chunk.index as usize] = true;
            transfer.bytes_transferred += chunk.data.len() as u64;

            let chunks_received_count = transfer.chunks_received.iter().filter(|&&x| x).count();
            info!(
                "📊 RECEIVER: Chunk {} marked as received for transfer {} ({}/{} chunks, {} bytes total)",
                chunk.index, transfer_id, chunks_received_count, transfer.chunks_received.len(), transfer.bytes_transferred
            );

            // Check if transfer is complete
            chunk.is_last || transfer.chunks_received.iter().all(|&received| received)
        };

        if is_complete {
            info!(
                "🎉 RECEIVER: Transfer {} is complete, finalizing...",
                transfer_id
            );
            self.complete_file_transfer(transfer_id).await?;
        } else {
            // Send acknowledgment through normal message channel
            if let Some(ref sender) = self.message_sender {
                let ack = Message::new(MessageType::FileChunkAck {
                    transfer_id,
                    chunk_index: chunk.index,
                });
                let _ = sender.send((peer_id, ack));
            }
        }

        Ok(())
    }

    async fn complete_file_transfer(&mut self, transfer_id: Uuid) -> Result<()> {
        let (file_path, expected_checksum, file_data, peer_id) = {
            let transfer = self
                .active_transfers
                .get_mut(&transfer_id)
                .ok_or_else(|| FileshareError::Transfer("Transfer not found".to_string()))?;

            // CRITICAL: Mark as completed IMMEDIATELY to prevent retries
            transfer.status = TransferStatus::Completed;
            transfer.last_activity = Instant::now();

            (
                transfer.file_path.clone(),
                transfer.metadata.checksum.clone(),
                transfer.received_data.clone(),
                transfer.peer_id,
            )
        };

        info!("📝 RECEIVER: Writing complete file to: {:?}", file_path);
        info!("📊 RECEIVER: File data size: {} bytes", file_data.len());

        // Write the complete file data at once
        std::fs::write(&file_path, &file_data).map_err(|e| {
            error!("Failed to write file {:?}: {}", file_path, e);
            FileshareError::FileOperation(format!("Failed to write file: {}", e))
        })?;

        info!("✅ RECEIVER: File written successfully to: {:?}", file_path);

        // Verify checksum if provided
        if !expected_checksum.is_empty() {
            let calculated_checksum = self.calculate_file_checksum(&file_path)?;
            if calculated_checksum != expected_checksum {
                error!(
                    "❌ RECEIVER: Checksum mismatch for transfer {}: expected {}, got {}",
                    transfer_id, expected_checksum, calculated_checksum
                );

                if let Some(transfer) = self.active_transfers.get_mut(&transfer_id) {
                    transfer.status = TransferStatus::Error("Checksum mismatch".to_string());
                }
                return Err(FileshareError::Transfer(
                    "File integrity check failed".to_string(),
                ));
            } else {
                info!(
                    "✅ RECEIVER: Checksum verified for transfer {}",
                    transfer_id
                );
            }
        }

        // Send completion acknowledgment to sender
        if let Some(ref sender) = self.message_sender {
            let completion_ack = Message::new(MessageType::TransferComplete {
                transfer_id,
                checksum: expected_checksum,
            });

            if let Err(e) = sender.send((peer_id, completion_ack)) {
                warn!("Failed to send completion acknowledgment: {}", e);
            } else {
                info!(
                    "✅ Sent completion acknowledgment for transfer {}",
                    transfer_id
                );
            }
        }

        info!(
            "🎉 RECEIVER: File transfer {} completed successfully: {:?}",
            transfer_id, file_path
        );

        // Show notification
        if let Some(transfer) = self.active_transfers.get(&transfer_id) {
            if let Err(e) = self.show_transfer_notification(transfer).await {
                warn!("Failed to show notification: {}", e);
            }
        }

        // CRITICAL: Remove completed transfer after a short delay to prevent immediate cleanup issues
        tokio::spawn({
            let transfer_id = transfer_id;
            async move {
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                // Note: We can't access self here, so we'll handle this in the cleanup method
            }
        });

        Ok(())
    }

    pub async fn handle_transfer_complete(
        &mut self,
        _peer_id: Uuid,
        transfer_id: Uuid,
        _checksum: String,
    ) -> Result<()> {
        info!("✅ Transfer {} completed by peer", transfer_id);
        if let Some(transfer) = self.active_transfers.get_mut(&transfer_id) {
            transfer.status = TransferStatus::Completed;
            transfer.last_activity = Instant::now();
        }

        if let Some(transfer) = self.active_transfers.get(&transfer_id) {
            if let Err(e) = self.show_transfer_notification(transfer).await {
                warn!("Failed to show notification: {}", e);
            }
        }
        Ok(())
    }

    pub async fn handle_transfer_error(
        &mut self,
        _peer_id: Uuid,
        transfer_id: Uuid,
        error: String,
    ) -> Result<()> {
        error!("❌ Transfer {} failed: {}", transfer_id, error);
        if let Some(transfer) = self.active_transfers.get_mut(&transfer_id) {
            transfer.status = TransferStatus::Error(error);
            transfer.last_activity = Instant::now();
        }
        Ok(())
    }

    fn get_save_path(&self, filename: &str, target_dir: Option<&str>) -> Result<PathBuf> {
        let save_dir = if let Some(target_dir_str) = target_dir {
            let target_path = PathBuf::from(target_dir_str);
            if target_path.exists() && target_path.is_dir() {
                info!("Using specified target directory: {:?}", target_path);
                target_path
            } else {
                warn!(
                    "Specified target directory {:?} doesn't exist, using default",
                    target_path
                );
                self.get_default_save_dir()
            }
        } else {
            self.get_default_save_dir()
        };

        if !save_dir.exists() {
            std::fs::create_dir_all(&save_dir).map_err(|e| {
                FileshareError::FileOperation(format!("Failed to create save directory: {}", e))
            })?;
        }

        let mut save_path = save_dir.join(filename);

        // Handle duplicate files
        let mut counter = 1;
        while save_path.exists() {
            let stem = std::path::Path::new(filename)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("file");

            let extension = std::path::Path::new(filename)
                .extension()
                .and_then(|s| s.to_str())
                .map(|s| format!(".{}", s))
                .unwrap_or_default();

            let new_filename = format!("{} ({}){}", stem, counter, extension);
            save_path = save_dir.join(new_filename);
            counter += 1;
        }

        info!("File will be saved to: {:?}", save_path);
        Ok(save_path)
    }

    fn get_default_save_dir(&self) -> PathBuf {
        if let Some(ref temp_dir) = self.settings.transfer.temp_dir {
            temp_dir.clone()
        } else {
            directories::UserDirs::new()
                .and_then(|dirs| dirs.download_dir().map(|d| d.to_path_buf()))
                .unwrap_or_else(|| {
                    directories::UserDirs::new()
                        .and_then(|dirs| dirs.document_dir().map(|d| d.to_path_buf()))
                        .unwrap_or_else(|| PathBuf::from("."))
                })
        }
    }

    fn calculate_file_checksum(&self, file_path: &PathBuf) -> Result<String> {
        use sha2::{Digest, Sha256};

        let file_data = std::fs::read(file_path).map_err(|e| {
            FileshareError::FileOperation(format!("Failed to read file for checksum: {}", e))
        })?;

        let mut hasher = Sha256::new();
        hasher.update(&file_data);
        Ok(format!("{:x}", hasher.finalize()))
    }

    async fn show_transfer_notification(&self, transfer: &FileTransfer) -> Result<()> {
        let message = match transfer.direction {
            TransferDirection::Incoming => {
                format!("Received file: {}", transfer.metadata.name)
            }
            TransferDirection::Outgoing => {
                format!("Sent file: {}", transfer.metadata.name)
            }
        };

        notify_rust::Notification::new()
            .summary("File Transfer Complete")
            .body(&message)
            .timeout(notify_rust::Timeout::Milliseconds(5000))
            .show()
            .map_err(|e| FileshareError::Unknown(format!("Notification error: {}", e)))?;

        Ok(())
    }

    pub fn get_active_transfers(&self) -> Vec<&FileTransfer> {
        self.active_transfers.values().collect()
    }

    pub fn get_transfer_progress(&self, transfer_id: Uuid) -> Option<f32> {
        self.active_transfers.get(&transfer_id).map(|transfer| {
            if transfer.metadata.size == 0 {
                1.0
            } else {
                transfer.bytes_transferred as f32 / transfer.metadata.size as f32
            }
        })
    }

    pub fn get_transfer_direction(&self, transfer_id: Uuid) -> Option<TransferDirection> {
        self.active_transfers
            .get(&transfer_id)
            .map(|t| t.direction.clone())
    }

    pub fn has_transfer(&self, transfer_id: Uuid) -> bool {
        self.active_transfers.contains_key(&transfer_id)
    }

    pub fn debug_active_transfers(&self) {
        info!("=== ACTIVE TRANSFERS DEBUG ===");
        for (transfer_id, transfer) in &self.active_transfers {
            info!(
                "Transfer {}: {:?} -> {:?} (Status: {:?}, Retry: {})",
                transfer_id,
                transfer.direction,
                transfer.file_path.file_name().unwrap_or_default(),
                transfer.status,
                transfer.retry_state.attempt_count
            );
        }
        info!("=== END TRANSFERS DEBUG ===");
    }
}
