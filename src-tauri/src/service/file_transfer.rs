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

        let metadata = FileMetadata::from_path_with_chunk_size_and_settings(
            &file_path, 
            chunk_size, 
            self.settings.transfer.compression_enabled
        )?.with_target_dir(target_dir);
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
        let use_parallel = settings.transfer.parallel_chunks > 1; // Enable parallel mode based on configuration

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
        let metadata = FileMetadata::from_path_with_chunk_size_and_settings(
            &file_path, 
            chunk_size, 
            settings.transfer.compression_enabled
        )?;
        let compression = metadata.compression;
        let use_streaming = metadata.streaming_mode;
        let total_chunks = metadata.total_chunks;

        // Get parallel chunks setting from configuration
        let parallel_chunks = settings.transfer.parallel_chunks;
        let use_parallel = settings.transfer.parallel_chunks > 1; // Enable parallel mode based on configuration

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
        // PERFORMANCE: Use high-speed streaming with batching
        let mut reader = StreamingFileReader::new(&file_path, chunk_size, compression).await?;
        let start_time = Instant::now();
        
        // OPTIMIZATION: Batch multiple chunks into single messages for better TCP efficiency
        const CHUNKS_PER_BATCH: usize = 8; // Send 8 chunks per message
        let mut chunk_batch = Vec::with_capacity(CHUNKS_PER_BATCH);
        let mut chunk_index = 0u64;
        let mut bytes_sent = 0u64;

        // CRITICAL: No artificial delays - stream as fast as possible
        while let Some((chunk_data, is_last)) = reader.read_next_chunk().await? {
            let chunk = TransferChunk {
                index: chunk_index,
                data: chunk_data,
                is_last,
                compressed: compression.is_some(),
                checksum: None,
            };
            
            bytes_sent += chunk.data.len() as u64;
            chunk_batch.push(chunk);
            chunk_index += 1;

            // Send batch when full or on last chunk
            if chunk_batch.len() >= CHUNKS_PER_BATCH || is_last {
                // OPTIMIZATION: Use batched message for better throughput
                let batch_message = Message::new(MessageType::FileChunkBatch {
                    transfer_id,
                    chunks: chunk_batch.clone(),
                });

                if let Err(e) = message_sender.send((peer_id, batch_message)) {
                    error!("❌ Failed to send chunk batch: {}", e);
                    return Err(FileshareError::Transfer(format!(
                        "Failed to send chunk batch: {}", e
                    )));
                }

                chunk_batch.clear();
                
                // Only log every 10th batch to reduce overhead
                if chunk_index % (CHUNKS_PER_BATCH as u64 * 10) == 0 || is_last {
                    let elapsed = start_time.elapsed().as_secs_f64();
                    let speed_mbps = if elapsed > 0.0 {
                        (bytes_sent as f64 / elapsed) / (1024.0 * 1024.0)
                    } else {
                        0.0
                    };
                    info!("📈 HIGH_SPEED: {} chunks sent, {:.1} MB/s", chunk_index, speed_mbps);
                }
            }

            if is_last { break; }
        }

        // Spawn background progress reporting to avoid blocking main transfer
        let progress_sender = message_sender.clone();
        let total_bytes = reader.progress().1;
        tokio::spawn(async move {
            let mut last_report = Instant::now();
            while last_report.elapsed() < Duration::from_secs(1) {
                if last_report.elapsed() > Duration::from_millis(1000) {
                    let elapsed = start_time.elapsed().as_secs_f64();
                    let speed_bps = if elapsed > 0.0 {
                        (bytes_sent as f64 / elapsed) as u64
                    } else {
                        0
                    };

                    let progress_msg = Message::new(MessageType::TransferProgress {
                        transfer_id,
                        bytes_transferred: bytes_sent,
                        chunks_completed: chunk_index,
                        speed_bps,
                        eta_seconds: None,
                    });

                    let _ = progress_sender.send((peer_id, progress_msg));
                    last_report = Instant::now();
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        });

        let checksum = reader.get_checksum();
        let elapsed = start_time.elapsed();
        let final_speed_mbps = (bytes_sent as f64 / elapsed.as_secs_f64()) / (1024.0 * 1024.0);
        
        info!(
            "🚀 SEQUENTIAL: Transfer {} complete - {} chunks, {:.1} MB sent in {:.2}s @ {:.1} MB/s, checksum: {}",
            transfer_id, chunk_index, bytes_sent as f64 / (1024.0 * 1024.0), 
            elapsed.as_secs_f64(), final_speed_mbps, checksum
        );

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
            "🚀 PARALLEL_STREAMING: Starting high-speed parallel transfer with {} streams",
            parallel_chunks
        );

        let start_time = Instant::now();
        
        // OPTIMIZATION: Larger buffer for better throughput
        let (chunk_tx, mut chunk_rx) = tokio::sync::mpsc::channel::<(u64, Vec<u8>, bool)>(parallel_chunks * 4);
        
        // High-speed producer with read-ahead
        let file_path_clone = file_path.clone();
        let chunk_size_clone = chunk_size;
        let compression_clone = compression.clone();
        
        let producer_handle = tokio::spawn(async move {
            let mut reader = StreamingFileReader::new(&file_path_clone, chunk_size_clone, compression_clone).await?;
            let mut chunk_index = 0u64;
            
            // PERFORMANCE: Stream chunks as fast as possible
            while let Some((chunk_data, is_last)) = reader.read_next_chunk().await? {
                if chunk_tx.send((chunk_index, chunk_data, is_last)).await.is_err() {
                    break; // Receiver dropped
                }
                chunk_index += 1;
                if is_last { break; }
            }
            
            Ok::<String, FileshareError>(reader.get_checksum())
        });

        let parallel_sender = ParallelChunkSender::new(
            message_sender.clone(),
            peer_id,
            transfer_id,
            parallel_chunks * 2, // Double the parallelism
        );

        let mut tracker = TransferTracker::new(transfer_id, total_chunks);
        let mut bytes_sent = 0u64;
        
        // OPTIMIZATION: Large batch processing for maximum throughput
        const LARGE_BATCH_SIZE: usize = 16; // Process 16 chunks at once
        let mut active_batch = Vec::with_capacity(LARGE_BATCH_SIZE);

        // High-speed chunk processing loop
        while let Some((chunk_index, chunk_data, is_last)) = chunk_rx.recv().await {
            bytes_sent += chunk_data.len() as u64;
            
            let chunk = TransferChunk {
                index: chunk_index,
                data: chunk_data,
                is_last,
                compressed: compression.is_some(),
                checksum: None,
            };
            
            active_batch.push((chunk_index, chunk));
            
            // Send large batches for maximum efficiency
            if active_batch.len() >= LARGE_BATCH_SIZE || is_last {
                let batch_indices: Vec<u64> = active_batch.iter().map(|(idx, _)| *idx).collect();
                
                tracker.mark_in_progress(&batch_indices);
                
                // CRITICAL: Use maximum batch size for TCP efficiency
                let failed_chunks = parallel_sender.send_chunks_batched(active_batch, 8).await?;
                
                // Update tracker with successful sends
                for &chunk_index in &batch_indices {
                    if !failed_chunks.contains(&chunk_index) {
                        tracker.mark_completed(chunk_index);
                    } else {
                        tracker.mark_failed(chunk_index);
                    }
                }
                
                active_batch = Vec::with_capacity(LARGE_BATCH_SIZE);
                
                // Minimal progress logging to avoid overhead
                if chunk_index % 80 == 0 || is_last { // Only every 80 chunks
                    let elapsed = start_time.elapsed().as_secs_f64();
                    let speed_mbps = if elapsed > 0.0 {
                        (bytes_sent as f64 / elapsed) / (1024.0 * 1024.0)
                    } else {
                        0.0
                    };
                    info!("⚡ PARALLEL: {} chunks @ {:.1} MB/s", chunk_index, speed_mbps);
                }
            }
            
            if is_last { break; }
        }
        
        // Get final checksum
        let file_checksum = match producer_handle.await {
            Ok(Ok(checksum)) => checksum,
            Ok(Err(e)) => {
                error!("Producer task error: {}", e);
                String::new()
            },
            Err(e) => {
                error!("Producer task panic: {}", e);
                String::new()
            }
        };

        // Fast retry for any failed chunks
        let pending = tracker.get_pending_chunks();
        if !pending.is_empty() {
            warn!("⚠️ Retrying {} failed chunks", pending.len());
            
            let mut reader = StreamingFileReader::new(&file_path, chunk_size, compression).await?;
            
            // Batch retry failed chunks for efficiency
            let mut retry_batch = Vec::new();
            for chunk_index in pending {
                if let Some((chunk_data, _)) = reader.read_chunk_at_index(chunk_index).await? {
                    let chunk = TransferChunk {
                        index: chunk_index,
                        data: chunk_data,
                        is_last: chunk_index == total_chunks - 1,
                        compressed: compression.is_some(),
                        checksum: None,
                    };
                    
                    retry_batch.push(chunk);
                    
                    // Send retry batch when full
                    if retry_batch.len() >= 8 {
                        let batch_message = Message::new(MessageType::FileChunkBatch {
                            transfer_id,
                            chunks: retry_batch.clone(),
                        });
                        let _ = message_sender.send((peer_id, batch_message));
                        retry_batch.clear();
                    }
                }
            }
            
            // Send remaining retry chunks
            if !retry_batch.is_empty() {
                let batch_message = Message::new(MessageType::FileChunkBatch {
                    transfer_id,
                    chunks: retry_batch,
                });
                let _ = message_sender.send((peer_id, batch_message));
            }
        }

        let elapsed = start_time.elapsed();
        let final_speed_mbps = (bytes_sent as f64 / elapsed.as_secs_f64()) / (1024.0 * 1024.0);

        info!(
            "🚀 PARALLEL: Transfer {} complete - {} chunks, {:.1} MB sent in {:.2}s @ {:.1} MB/s, checksum: {}",
            transfer_id, tracker.completed_chunks.len(), bytes_sent as f64 / (1024.0 * 1024.0),
            elapsed.as_secs_f64(), final_speed_mbps, file_checksum
        );

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
            let buffer = vec![0u8; metadata.size as usize]; // Keep old behavior for small files
            info!(
                "🔧 BUFFER_INIT: Non-streaming mode, allocated buffer of {} bytes (all zeros)",
                buffer.len()
            );
            buffer
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
        if chunk.index % 10 == 0 || chunk.is_last || chunk.index < 5 {
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
                
                // CRITICAL FIX: Mark chunk as received in chunks_received array for streaming mode
                transfer.chunks_received[chunk.index as usize] = true;

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
                
                // Skip duplicate chunks
                if transfer.chunks_received[chunk.index as usize] {
                    warn!(
                        "⚠️ RECEIVER: Skipping duplicate chunk {} for transfer {}",
                        chunk.index, transfer_id
                    );
                } else {
                    // Decompress chunk data if it's compressed
                let compression_type = transfer.metadata.compression;
                let decompressed_data = if chunk.compressed && compression_type.is_some() {
                    let decompressed = Self::decompress_chunk_data_static(&chunk.data, compression_type.unwrap())?;
                    
                    // Debug: Compare compressed vs decompressed for first few chunks
                    if chunk.index < 5 || chunk.index >= 25 {
                        let compressed_first = if chunk.data.len() >= 8 { 
                            format!("{:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}", 
                                chunk.data[0], chunk.data[1], chunk.data[2], chunk.data[3],
                                chunk.data[4], chunk.data[5], chunk.data[6], chunk.data[7]) 
                        } else { 
                            format!("{:?}", &chunk.data[..chunk.data.len().min(8)]) 
                        };
                        let compressed_last = if chunk.data.len() >= 8 { 
                            let len = chunk.data.len();
                            format!("{:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}", 
                                chunk.data[len-8], chunk.data[len-7], chunk.data[len-6], chunk.data[len-5],
                                chunk.data[len-4], chunk.data[len-3], chunk.data[len-2], chunk.data[len-1]) 
                        } else { 
                            format!("{:?}", chunk.data) 
                        };
                        let decompressed_first = if decompressed.len() >= 8 { 
                            format!("{:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}", 
                                decompressed[0], decompressed[1], decompressed[2], decompressed[3],
                                decompressed[4], decompressed[5], decompressed[6], decompressed[7]) 
                        } else { 
                            format!("{:?}", &decompressed[..decompressed.len().min(8)]) 
                        };
                        debug!(
                            "🔧 DECOMP_DEBUG: Chunk {} - Compressed size: {}, first: {}, last: {} | Decompressed size: {}, first: {}",
                            chunk.index, chunk.data.len(), compressed_first, compressed_last, decompressed.len(), decompressed_first
                        );
                    }
                    
                    decompressed
                } else {
                    if chunk.index < 5 || chunk.index >= 25 {
                        let uncompressed_first = if chunk.data.len() >= 8 { 
                            format!("{:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}", 
                                chunk.data[0], chunk.data[1], chunk.data[2], chunk.data[3],
                                chunk.data[4], chunk.data[5], chunk.data[6], chunk.data[7]) 
                        } else { 
                            format!("{:?}", &chunk.data[..chunk.data.len().min(8)]) 
                        };
                        debug!(
                            "🔧 UNCOMP_DEBUG: Chunk {} - Size: {}, first bytes: {}",
                            chunk.index, chunk.data.len(), uncompressed_first
                        );
                    }
                    chunk.data.clone()
                };
                
                let chunk_size = transfer.metadata.chunk_size as u64;
                let expected_offset = chunk.index * chunk_size;
                let actual_end_offset = expected_offset + decompressed_data.len() as u64;
                let actual_file_size = transfer.metadata.size;

                // For the last chunk, allow it to extend slightly beyond file size (common with compression)
                // but validate that the start position is within bounds
                if expected_offset >= actual_file_size {
                    return Err(FileshareError::Transfer(format!(
                        "Chunk {} starts beyond file size (offset: {}, file size: {})",
                        chunk.index, expected_offset, actual_file_size
                    )));
                }
                
                // For non-last chunks, validate end offset doesn't exceed file size
                if !chunk.is_last && actual_end_offset > actual_file_size {
                    return Err(FileshareError::Transfer(format!(
                        "Chunk {} too large: exceeds file size (offset: {}, file size: {})",
                        chunk.index, actual_end_offset, actual_file_size
                    )));
                }
                
                // For last chunk, calculate the actual bytes that should be copied
                let bytes_to_copy = if chunk.is_last {
                    std::cmp::min(decompressed_data.len(), (actual_file_size - expected_offset) as usize)
                } else {
                    decompressed_data.len()
                };

                    // Copy decompressed chunk data into the buffer
                    let start_idx = expected_offset as usize;
                    let end_idx = expected_offset as usize + bytes_to_copy;
                    
                    // Debug: Log buffer write operations for first few chunks
                    if chunk.index < 5 || chunk.index >= 25 {
                        debug!(
                            "🔧 BUFFER_WRITE: Chunk {} -> buffer[{}..{}] ({} bytes), compressed_size: {}, decompressed_size: {}",
                            chunk.index, start_idx, end_idx, bytes_to_copy, chunk.data.len(), decompressed_data.len()
                        );
                    }
                    
                    transfer.received_data[start_idx..end_idx].copy_from_slice(&decompressed_data[..bytes_to_copy]);
                    transfer.bytes_transferred += bytes_to_copy as u64;
                    
                    // Debug: Verify buffer content for first few chunks
                    if chunk.index < 5 || chunk.index >= 25 {
                        let buffer_slice = &transfer.received_data[start_idx..end_idx];
                        let first_bytes = if buffer_slice.len() >= 8 { 
                            format!("{:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}", 
                                buffer_slice[0], buffer_slice[1], buffer_slice[2], buffer_slice[3],
                                buffer_slice[4], buffer_slice[5], buffer_slice[6], buffer_slice[7]) 
                        } else { 
                            format!("{:?}", buffer_slice) 
                        };
                        debug!(
                            "🔧 BUFFER_VERIFY: Chunk {} written to file position {} (buffer[{}..{}]), first 8 bytes: {}",
                            chunk.index, expected_offset, start_idx, end_idx, first_bytes
                        );
                    }
                    
                    // Mark chunk as received
                    transfer.chunks_received[chunk.index as usize] = true;
                }
            }

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

            // Calculate checksum on the data buffer before writing to disk
            let calculated_checksum = {
                use sha2::{Digest, Sha256};
                let mut hasher = Sha256::new();
                hasher.update(&file_data);
                let checksum = format!("{:x}", hasher.finalize());
                
                // Debug: Log first and last 32 bytes of assembled file data
                let first_bytes = if file_data.len() >= 32 { 
                    format!("{:?}", &file_data[..32]) 
                } else { 
                    format!("{:?}", &file_data[..file_data.len().min(32)]) 
                };
                let last_bytes = if file_data.len() >= 32 { 
                    format!("{:?}", &file_data[file_data.len()-32..]) 
                } else { 
                    format!("{:?}", &file_data[file_data.len().saturating_sub(32)..]) 
                };
                
                debug!(
                    "🔍 CHECKSUM_DEBUG: File data checksum: {}, size: {}, first_32: {}, last_32: {}",
                    checksum, file_data.len(), first_bytes, last_bytes
                );
                
                checksum
            };

            std::fs::write(&file_path, &file_data).map_err(|e| {
                error!("Failed to write file {:?}: {}", file_path, e);
                FileshareError::FileOperation(format!("Failed to write file: {}", e))
            })?;

            info!("✅ RECEIVER: File written successfully to: {:?}", file_path);

            calculated_checksum
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
    
    // Helper method to decompress chunk data (used in non-streaming mode)
    fn decompress_chunk_data_static(data: &[u8], compression: crate::network::protocol::CompressionType) -> Result<Vec<u8>> {
        use crate::network::protocol::CompressionType;
        
        match compression {
            CompressionType::None => Ok(data.to_vec()),
            CompressionType::Zstd => {
                zstd::decode_all(data)
                    .map_err(|e| FileshareError::Transfer(format!("Zstd decompression failed: {}", e)))
            }
            CompressionType::Lz4 => {
                lz4::block::decompress(data, None)
                    .map_err(|e| FileshareError::Transfer(format!("Lz4 decompression failed: {}", e)))
            }
        }
    }
}
