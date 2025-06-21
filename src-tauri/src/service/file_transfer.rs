use crate::{config::Settings, network::protocol::*, FileshareError, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::time::sleep;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::service::parallel_transfer::{ChunkBatcher, ParallelChunkSender, TransferTracker};
use crate::service::streaming::{
    calculate_adaptive_chunk_size, StreamingFileReader, StreamingFileWriter, TransferProgress,
};

// Transfer constants
const MAX_RETRY_ATTEMPTS: u32 = 3;
const RETRY_BASE_DELAY_MS: u64 = 1000; // 1 second base delay
const MAX_RETRY_DELAY_MS: u64 = 8000; // 8 seconds max delay
const TRANSFER_TIMEOUT_SECONDS: u64 = 3600; // Base timeout: 1 hour per transfer
const MIN_TRANSFER_TIMEOUT_SECONDS: u64 = 300; // Minimum 5 minutes
const MAX_TRANSFER_TIMEOUT_SECONDS: u64 = 28800; // Maximum 8 hours
const CHUNK_TIMEOUT_SECONDS: u64 = 300; // 5 minutes per chunk (more reasonable for large files)
const PROGRESS_UPDATE_INTERVAL_MS: u64 = 500; // Update progress every 500ms

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
    pub retry_state: RetryState,
    pub last_activity: Instant,
    // Streaming fields
    pub streaming_writer: Option<Box<StreamingFileWriter>>,
    pub chunks_completed: u64,
    pub speed_bps: u64,
    // Resume fields
    pub completed_chunks: Vec<u64>,
    pub resume_token: Option<String>,
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
    // Calculate adaptive timeout based on file size
    fn calculate_transfer_timeout(file_size: u64) -> u64 {
        // Base calculation: 1 hour + 1 minute per 100MB
        let size_mb = file_size / (1024 * 1024);
        let adaptive_timeout = TRANSFER_TIMEOUT_SECONDS + (size_mb * 60 / 100);
        
        // Clamp between min and max values
        adaptive_timeout.clamp(MIN_TRANSFER_TIMEOUT_SECONDS, MAX_TRANSFER_TIMEOUT_SECONDS)
    }
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

    // Validate file before transfer
    pub fn validate_file_size(&self, file_path: &PathBuf) -> Result<()> {
        let metadata = std::fs::metadata(file_path)
            .map_err(|e| FileshareError::FileOperation(format!("Cannot access file: {}", e)))?;

        let file_size = metadata.len();

        if file_size == 0 {
            return Err(FileshareError::FileOperation(
                "Cannot transfer empty files".to_string(),
            ));
        }

        // No size limit anymore - streaming handles large files
        info!("File size: {} MB", file_size / (1024 * 1024));

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

            // Check for overall transfer timeout (adaptive based on file size)
            let adaptive_timeout = Self::calculate_transfer_timeout(transfer.metadata.size);
            if now.duration_since(transfer.created_at).as_secs() > adaptive_timeout {
                error!(
                    "⏰ Transfer {} timed out after {} seconds (adaptive timeout: {}s for {}MB file)",
                    transfer_id, 
                    now.duration_since(transfer.created_at).as_secs(),
                    adaptive_timeout,
                    transfer.metadata.size / (1024 * 1024)
                );
                timed_out_transfers.push(*transfer_id);
                continue;
            }

            // Check for inactive transfers (no activity for chunk timeout)
            // But only if the transfer has been running for at least twice the chunk timeout
            // to prevent recovery loops on newly started transfers
            let transfer_age = now.duration_since(transfer.created_at).as_secs();
            let inactivity_duration = now.duration_since(transfer.last_activity).as_secs();
            
            if inactivity_duration > CHUNK_TIMEOUT_SECONDS && transfer_age > (CHUNK_TIMEOUT_SECONDS * 2) {
                match transfer.status {
                    TransferStatus::Active | TransferStatus::Pending => {
                        warn!(
                            "⚠️ Transfer {} inactive for {} seconds (transfer age: {}s), marking for recovery",
                            transfer_id, inactivity_duration, transfer_age
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

        // Determine chunk size - adaptive or fixed
        let file_metadata = std::fs::metadata(&file_path)?;
        let file_size = file_metadata.len();

        let chunk_size = if self.settings.transfer.adaptive_chunk_size {
            calculate_adaptive_chunk_size(file_size)
        } else {
            self.settings.transfer.chunk_size
        };

        info!(
            "📊 Using chunk size: {} KB for file size: {} MB",
            chunk_size / 1024,
            file_size / (1024 * 1024)
        );

        let metadata = FileMetadata::from_path_with_chunk_size(&file_path, chunk_size)?
            .with_target_dir(target_dir);
        let transfer_id = Uuid::new_v4();

        info!(
            "🚀 SEND_FILE_METADATA: Created transfer {} for peer {}", transfer_id, peer_id
        );
        info!(
            "📊 METADATA: file_size={} bytes, calculated_chunk_size={} bytes, stored_chunk_size={} bytes, total_chunks={}",
            metadata.size, chunk_size, metadata.chunk_size, metadata.total_chunks
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
            streaming_writer: None,
            chunks_completed: 0,
            speed_bps: 0,
            completed_chunks: Vec::new(),
            resume_token: None,
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
        let (file_path, peer_id, metadata) = {
            let transfer = self
                .active_transfers
                .get_mut(&transfer_id)
                .ok_or_else(|| FileshareError::Transfer("Transfer not found".to_string()))?;

            transfer.status = TransferStatus::Active;
            transfer.last_activity = Instant::now();

            // Use the original metadata to ensure consistency
            (transfer.file_path.clone(), transfer.peer_id, transfer.metadata.clone())
        };

        let message_sender = self
            .message_sender
            .clone()
            .ok_or_else(|| FileshareError::Transfer("Message sender not configured".to_string()))?;

        let settings = self.settings.clone();

        tokio::spawn(async move {
            if let Err(e) =
                Self::send_file_chunks_with_metadata(message_sender, peer_id, transfer_id, file_path, metadata, settings)
                    .await
            {
                error!("Failed to send file chunks: {}", e);
            }
        });

        Ok(())
    }

    async fn send_file_chunks_with_metadata(
        message_sender: MessageSender,
        peer_id: Uuid,
        transfer_id: Uuid,
        file_path: PathBuf,
        metadata: FileMetadata,
        settings: Arc<Settings>,
    ) -> Result<()> {
        info!(
            "🚀 STREAMING_SENDER: Starting streaming file transfer {} to peer {}",
            transfer_id, peer_id
        );
        info!("📄 File path: {:?}, using original metadata - chunk size: {}, total chunks: {}", 
              file_path, metadata.chunk_size, metadata.total_chunks);

        let chunk_size = metadata.chunk_size;
        let compression = metadata.compression;
        let use_streaming = metadata.streaming_mode;
        let total_chunks = metadata.total_chunks;

        // Get parallel chunks setting from configuration
        let parallel_chunks = settings.transfer.parallel_chunks;
        let use_parallel = false; // Temporarily disable parallel mode to test sequential fixes

        info!(
            "📊 File: {} bytes, streaming: {}, compression: {:?}, parallel: {} (chunks: {})",
            metadata.size, use_streaming, compression, use_parallel, parallel_chunks
        );

        if use_parallel {
            Self::send_file_chunks_parallel(
                message_sender,
                peer_id,
                transfer_id,
                file_path,
                chunk_size,
                compression,
                parallel_chunks,
                total_chunks,
            )
            .await
        } else {
            Self::send_file_chunks_sequential(
                message_sender,
                peer_id,
                transfer_id,
                file_path,
                chunk_size,
                compression,
            )
            .await
        }
    }

    async fn send_file_chunks(
        message_sender: MessageSender,
        peer_id: Uuid,
        transfer_id: Uuid,
        file_path: PathBuf,
        chunk_size: usize,
        settings: Arc<Settings>,
    ) -> Result<()> {
        info!(
            "🚀 STREAMING_SENDER: Starting streaming file transfer {} to peer {}",
            transfer_id, peer_id
        );
        info!("📄 File path: {:?}, chunk size: {}", file_path, chunk_size);

        // Get file metadata to determine compression
        let metadata = FileMetadata::from_path_with_chunk_size(&file_path, chunk_size)?;
        let compression = metadata.compression;
        let use_streaming = metadata.streaming_mode;
        let total_chunks = metadata.total_chunks;

        // Get parallel chunks setting from configuration
        let parallel_chunks = settings.transfer.parallel_chunks;
        let use_parallel = false; // Temporarily disable parallel mode to test sequential fixes

        info!(
            "📊 File: {} bytes, streaming: {}, compression: {:?}, parallel: {} (chunks: {})",
            metadata.size, use_streaming, compression, use_parallel, parallel_chunks
        );

        if use_parallel {
            Self::send_file_chunks_parallel(
                message_sender,
                peer_id,
                transfer_id,
                file_path,
                chunk_size,
                compression,
                parallel_chunks,
                total_chunks,
            )
            .await
        } else {
            Self::send_file_chunks_sequential(
                message_sender,
                peer_id,
                transfer_id,
                file_path,
                chunk_size,
                compression,
            )
            .await
        }
    }

    async fn send_file_chunks_sequential(
        message_sender: MessageSender,
        peer_id: Uuid,
        transfer_id: Uuid,
        file_path: PathBuf,
        chunk_size: usize,
        compression: Option<CompressionType>,
    ) -> Result<()> {
        // Create streaming reader
        let mut reader = StreamingFileReader::new(&file_path, chunk_size, compression).await?;

        let mut chunk_index = 0u64;
        let mut last_progress_update = Instant::now();
        let start_time = Instant::now();
        let mut bytes_sent = 0u64;

        // Stream file chunks sequentially
        while let Some((chunk_data, is_last)) = reader.read_next_chunk().await? {
            let compressed = compression.is_some();

            let chunk = TransferChunk {
                index: chunk_index,
                data: chunk_data,
                is_last,
                compressed,
                checksum: None,
            };

            let message = Message::new(MessageType::FileChunk { transfer_id, chunk });

            // Send chunk
            if let Err(e) = message_sender.send((peer_id, message)) {
                error!("❌ Failed to send chunk {}: {}", chunk_index, e);
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

            let (current_bytes, total_bytes) = reader.progress();
            bytes_sent = current_bytes;
            chunk_index += 1;

            // Send progress updates periodically
            if last_progress_update.elapsed() > Duration::from_millis(PROGRESS_UPDATE_INTERVAL_MS) {
                let elapsed = start_time.elapsed().as_secs_f64();
                let speed_bps = if elapsed > 0.0 {
                    (bytes_sent as f64 / elapsed) as u64
                } else {
                    0
                };
                let eta_seconds = if speed_bps > 0 {
                    Some((total_bytes - bytes_sent) / speed_bps)
                } else {
                    None
                };

                let progress_msg = Message::new(MessageType::TransferProgress {
                    transfer_id,
                    bytes_transferred: bytes_sent,
                    chunks_completed: chunk_index,
                    speed_bps,
                    eta_seconds,
                });

                let _ = message_sender.send((peer_id, progress_msg));
                last_progress_update = Instant::now();
            }
        }

        // Get final checksum for logging
        let checksum = reader.get_checksum();

        info!(
            "✅ SEQUENTIAL: All chunks sent for transfer {} - {} chunks, {} bytes, checksum: {} (waiting for receiver confirmation)",
            transfer_id, chunk_index, bytes_sent, checksum
        );

        // Note: We don't send TransferComplete here - the receiver will send it to us
        // when it has successfully received and written all chunks
        
        Ok(())
    }

    async fn send_file_chunks_parallel(
        message_sender: MessageSender,
        peer_id: Uuid,
        transfer_id: Uuid,
        file_path: PathBuf,
        chunk_size: usize,
        compression: Option<CompressionType>,
        parallel_chunks: usize,
        total_chunks: u64,
    ) -> Result<()> {
        info!(
            "🚀 PARALLEL: Starting parallel transfer with {} concurrent chunks",
            parallel_chunks
        );

        // Create parallel sender and tracker
        let parallel_sender = ParallelChunkSender::new(
            message_sender.clone(),
            peer_id,
            transfer_id,
            parallel_chunks,
        );

        let mut tracker = TransferTracker::new(transfer_id, total_chunks);
        let mut batcher = ChunkBatcher::new(total_chunks, parallel_chunks);

        // Create multiple readers for parallel access
        let mut readers = Vec::new();
        for _ in 0..parallel_chunks {
            let reader = StreamingFileReader::new(&file_path, chunk_size, compression).await?;
            readers.push(reader);
        }

        let start_time = Instant::now();
        let mut last_progress_update = Instant::now();

        // Process chunks in batches
        while let Some(batch) = batcher.next_batch() {
            tracker.mark_in_progress(&batch);

            let mut chunks_to_send = Vec::new();

            // Read chunks for this batch
            for (i, &chunk_index) in batch.iter().enumerate() {
                let reader_index = i % readers.len();
                let _reader_len = readers.len();

                // Read chunk at the specific index (with proper seeking)
                if let Some((chunk_data, _is_last)) =
                    readers[reader_index].read_chunk_at_index(chunk_index).await?
                {
                    let chunk = TransferChunk {
                        index: chunk_index,
                        data: chunk_data,
                        is_last: chunk_index == total_chunks - 1,
                        compressed: compression.is_some(),
                        checksum: None,
                    };
                    chunks_to_send.push((chunk_index, chunk));
                }
            }

            // Send chunks in parallel
            let failed_chunks = parallel_sender.send_chunks_parallel(chunks_to_send).await?;

            // Update tracker
            for &chunk_index in &batch {
                if !failed_chunks.contains(&chunk_index) {
                    tracker.mark_completed(chunk_index);
                } else {
                    tracker.mark_failed(chunk_index);
                }
            }

            // Send progress update
            if last_progress_update.elapsed() > Duration::from_millis(PROGRESS_UPDATE_INTERVAL_MS) {
                let elapsed = start_time.elapsed().as_secs_f64();
                let bytes_transferred = tracker.completed_chunks.len() as u64 * chunk_size as u64;
                let speed_bps = if elapsed > 0.0 {
                    (bytes_transferred as f64 / elapsed) as u64
                } else {
                    0
                };

                let progress_msg = Message::new(MessageType::TransferProgress {
                    transfer_id,
                    bytes_transferred,
                    chunks_completed: tracker.completed_chunks.len() as u64,
                    speed_bps,
                    eta_seconds: None,
                });

                let _ = message_sender.send((peer_id, progress_msg));
                last_progress_update = Instant::now();

                info!(
                    "📊 PARALLEL: Progress {:.1}% ({}/{} chunks)",
                    tracker.progress_percentage(),
                    tracker.completed_chunks.len(),
                    total_chunks
                );
            }
        }

        // Retry failed chunks if any
        let pending = tracker.get_pending_chunks();
        if !pending.is_empty() {
            warn!("⚠️ PARALLEL: Retrying {} failed chunks", pending.len());
            // In a real implementation, we'd retry the failed chunks here
        }

        // Calculate final checksum for logging (simplified - in reality we'd need to combine all readers)
        let checksum = readers.into_iter().next().unwrap().get_checksum();

        info!(
            "✅ PARALLEL: All chunks sent for transfer {} - {} chunks, checksum: {} (waiting for receiver confirmation)",
            transfer_id,
            tracker.completed_chunks.len(),
            checksum
        );

        // Note: We don't send TransferComplete here - the receiver will send it to us
        // when it has successfully received and written all chunks

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

        // Use the chunk count from metadata
        let expected_chunks = metadata.total_chunks as usize;

        info!(
            "📦 RECEIVER: Transfer {} expects {} chunks of max {} bytes each",
            transfer_id, expected_chunks, metadata.chunk_size
        );

        // Determine if we should use streaming based on file size
        let use_streaming = metadata.streaming_mode;
        let received_data = if use_streaming {
            Vec::new() // No pre-allocation for streaming
        } else {
            vec![0u8; metadata.size as usize] // Keep old behavior for small files
        };

        // Create streaming writer if needed
        let streaming_writer = if use_streaming {
            let decompression = metadata.compression;
            let writer = StreamingFileWriter::new(
                &save_path,
                metadata.size,
                metadata.chunk_size,
                metadata.total_chunks,
                decompression,
            )
            .await?;
            Some(Box::new(writer))
        } else {
            None
        };

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
            streaming_writer,
            chunks_completed: 0,
            speed_bps: 0,
            completed_chunks: Vec::new(),
            resume_token: None,
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
        // Only log every 10th chunk to reduce noise, or important chunks
        if chunk.index % 10 == 0 || chunk.is_last {
            info!(
                "📥 RECEIVER: Chunk {} for transfer {} ({} bytes, compressed: {}, is_last: {})",
                chunk.index,
                transfer_id,
                chunk.data.len(),
                chunk.compressed,
                chunk.is_last
            );
        }

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

        let (is_complete, debug_info) = {
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

            // Validate chunk index
            if chunk.index >= transfer.chunks_received.len() as u64 {
                error!(
                    "❌ RECEIVER: Chunk index {} out of bounds for transfer {}",
                    chunk.index, transfer_id
                );
                return Err(FileshareError::Transfer(format!(
                    "Chunk index {} out of bounds (expected max {})",
                    chunk.index,
                    transfer.chunks_received.len() - 1
                )));
            }

            // Handle streaming vs non-streaming transfers
            if let Some(ref mut writer) = transfer.streaming_writer {
                // Streaming mode - write chunk directly to file
                writer
                    .write_chunk(chunk.index, chunk.data.clone(), chunk.compressed)
                    .await?;

                let (bytes_written, total_bytes) = writer.progress();
                transfer.bytes_transferred = bytes_written;
                transfer.chunks_completed += 1;

                // Only log progress every 50 chunks or on completion
                if transfer.chunks_completed % 50 == 0 || chunk.is_last {
                    let progress_pct = (bytes_written as f64 / total_bytes as f64 * 100.0);
                    info!(
                        "📊 TRANSFER_PROGRESS: {:.1}% complete - {}/{} chunks ({:.1}MB/{:.1}MB)",
                        progress_pct,
                        transfer.chunks_completed,
                        transfer.metadata.total_chunks,
                        bytes_written as f64 / (1024.0 * 1024.0),
                        total_bytes as f64 / (1024.0 * 1024.0)
                    );
                }
            } else {
                // Non-streaming mode - accumulate in memory
                let chunk_size = transfer.metadata.chunk_size as u64;
                let expected_offset = chunk.index * chunk_size;
                let actual_end_offset = expected_offset + chunk.data.len() as u64;
                let expected_file_size = transfer.received_data.len() as u64;

                if actual_end_offset > expected_file_size {
                    return Err(FileshareError::Transfer(format!(
                        "Chunk {} too large: exceeds file size",
                        chunk.index
                    )));
                }

                // Copy chunk data directly into the buffer
                let start_idx = expected_offset as usize;
                let end_idx = actual_end_offset as usize;
                transfer.received_data[start_idx..end_idx].copy_from_slice(&chunk.data);
                transfer.bytes_transferred += chunk.data.len() as u64;
            }

            // Mark chunk as received
            transfer.chunks_received[chunk.index as usize] = true;

            // Check if transfer is complete - ALL chunks must be received
            let chunks_received_count = transfer.chunks_received.iter().filter(|&&b| b).count();
            let total_chunks = transfer.metadata.total_chunks as usize;
            let is_complete = chunks_received_count == total_chunks;
            
            // Enhanced debugging to track which chunks are missing
            if chunk.index % 10 == 0 || chunk.is_last || is_complete {
                let missing_chunks: Vec<usize> = transfer.chunks_received
                    .iter()
                    .enumerate()
                    .filter(|(_, &received)| !received)
                    .map(|(idx, _)| idx)
                    .collect();
                
                info!(
                    "📊 CHUNK_STATUS: Transfer {} - Received {}/{} chunks, Missing: {:?}",
                    transfer_id, chunks_received_count, total_chunks, 
                    if missing_chunks.len() <= 10 { format!("{:?}", missing_chunks) } 
                    else { format!("{} chunks missing", missing_chunks.len()) }
                );
            }
            
            // Capture debugging info before releasing the transfer reference
            let debug_info = if is_complete {
                Some((chunks_received_count, total_chunks as u64))
            } else {
                None
            };
            
            (is_complete, debug_info)
        };

        if is_complete {
            // Log completion details for debugging
            if let Some((chunks_received_count, total_chunks)) = debug_info {
                info!(
                    "🎉 RECEIVER: Transfer {} is complete! Received {}/{} chunks, finalizing...",
                    transfer_id, chunks_received_count, total_chunks
                );
            }
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
        let (file_path, expected_checksum, peer_id, is_streaming) = {
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
                transfer.peer_id,
                transfer.streaming_writer.is_some(),
            )
        };

        info!("📝 RECEIVER: Completing file transfer to: {:?}", file_path);

        // Handle streaming vs non-streaming completion
        let calculated_checksum = if is_streaming {
            // Streaming mode - finalize the writer
            let writer = self
                .active_transfers
                .get_mut(&transfer_id)
                .and_then(|t| t.streaming_writer.take())
                .ok_or_else(|| {
                    FileshareError::Transfer("Streaming writer not found".to_string())
                })?;

            let checksum = writer.finalize().await?;
            info!(
                "✅ STREAMING: File written successfully with checksum: {}",
                checksum
            );
            checksum
        } else {
            // Non-streaming mode - write from memory buffer
            let file_data = self
                .active_transfers
                .get(&transfer_id)
                .map(|t| t.received_data.clone())
                .ok_or_else(|| FileshareError::Transfer("Transfer data not found".to_string()))?;

            info!("📊 RECEIVER: File data size: {} bytes", file_data.len());

            std::fs::write(&file_path, &file_data).map_err(|e| {
                error!("Failed to write file {:?}: {}", file_path, e);
                FileshareError::FileOperation(format!("Failed to write file: {}", e))
            })?;

            info!("✅ RECEIVER: File written successfully to: {:?}", file_path);

            // Calculate checksum
            self.calculate_file_checksum(&file_path)?
        };

        // Verify checksum if provided
        if !expected_checksum.is_empty() {
            info!(
                "🔍 CHECKSUM DEBUG: Transfer {} - Expected: {}, Calculated: {}, File size: {} bytes",
                transfer_id, expected_checksum, calculated_checksum, 
                tokio::fs::metadata(&file_path).await.map(|m| m.len()).unwrap_or(0)
            );
            
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
        if self.active_transfers.is_empty() {
            info!("📋 TRANSFER_STATUS: No active transfers");
            return;
        }
        
        info!("📋 TRANSFER_STATUS: {} active transfers", self.active_transfers.len());
        for (id, transfer) in &self.active_transfers {
            let progress_pct = if transfer.metadata.size > 0 {
                (transfer.bytes_transferred as f64 / transfer.metadata.size as f64 * 100.0)
            } else {
                0.0
            };
            
            let chunks_received_count = transfer.chunks_received.iter().filter(|&&b| b).count();
            let age_secs = transfer.created_at.elapsed().as_secs();
            let inactive_secs = transfer.last_activity.elapsed().as_secs();
            
            info!(
                "  📁 {}: {:.1}% | {} | {:.1}MB/{:.1}MB | {}/{} chunks | Age: {}s | Inactive: {}s | Retry: {} | {:?}",
                id.to_string().split('-').next().unwrap_or("unknown"),
                progress_pct,
                match &transfer.status {
                    TransferStatus::Active => "🟢 ACTIVE",
                    TransferStatus::Pending => "🟡 PENDING", 
                    TransferStatus::Paused => "⏸️ PAUSED",
                    TransferStatus::Completed => "✅ DONE",
                    TransferStatus::Error(e) => "❌ ERROR",
                    TransferStatus::Cancelled => "🚫 CANCELLED",
                },
                transfer.bytes_transferred as f64 / (1024.0 * 1024.0),
                transfer.metadata.size as f64 / (1024.0 * 1024.0),
                chunks_received_count,
                transfer.metadata.total_chunks,
                age_secs,
                inactive_secs,
                transfer.retry_state.attempt_count,
                transfer.direction
            );
        }
    }
}
