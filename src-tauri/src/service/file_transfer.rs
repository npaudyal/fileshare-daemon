use crate::service::{
    progress::TransferProgress,
    streaming::{StreamingFileReader, StreamingFileWriter},
};
use crate::{
    config::Settings, error::StreamingError, network::protocol::*, FileshareError, Result,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::time::sleep;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

// REMOVED: Phase 1 constants - NO MORE LIMITS!
// const MAX_FILE_SIZE_PHASE1: u64 = 100 * 1024 * 1024; // DELETED!

// NEW: Enhanced constants for streaming
const MAX_RETRY_ATTEMPTS: u32 = 3;
const RETRY_BASE_DELAY_MS: u64 = 1000;
const MAX_RETRY_DELAY_MS: u64 = 8000;
const TRANSFER_TIMEOUT_SECONDS: u64 = 3600; // 1 hour for large files
const CHUNK_TIMEOUT_SECONDS: u64 = 30;
const PROGRESS_UPDATE_INTERVAL_MS: u64 = 500; // Report progress every 500ms

// Use simple message sender instead of DirectPeerSender
pub type MessageSender = mpsc::UnboundedSender<(Uuid, Message)>;

// Enhanced retry state management
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
    pub received_data: Vec<u8>, // Only used for non-streaming transfers
    pub created_at: Instant,
    pub retry_state: RetryState,
    pub last_activity: Instant,
    pub progress: TransferProgress, // NEW: Enhanced progress tracking
    pub is_streaming: bool,         // NEW: Streaming mode flag
    pub streaming_writer: Option<StreamingFileWriter>, // NEW: Streaming writer
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

    // NEW: Enhanced file size validation with streaming support
    pub fn validate_file_for_transfer(&self, file_path: &PathBuf) -> Result<()> {
        let metadata = std::fs::metadata(file_path)
            .map_err(|e| FileshareError::FileOperation(format!("Cannot access file: {}", e)))?;

        let file_size = metadata.len();

        if file_size == 0 {
            return Err(FileshareError::FileOperation(
                "Cannot transfer empty files".to_string(),
            ));
        }

        // Check if we have enough memory for non-streaming mode
        if !self.settings.streaming.enable_streaming_mode
            && file_size > (self.settings.streaming.max_memory_buffer_mb as u64 * 1024 * 1024)
        {
            return Err(StreamingError::FileTooLargeForMemory { size: file_size }.into());
        }

        info!("✅ File validation passed: {} bytes", file_size);
        Ok(())
    }

    // Enhanced send_file with streaming support
    pub async fn send_file_with_validation(
        &mut self,
        peer_id: Uuid,
        file_path: PathBuf,
    ) -> Result<()> {
        info!(
            "🚀 STREAMING_SEND: Starting validated file transfer to {}: {:?}",
            peer_id, file_path
        );

        // Validate file before transfer
        self.validate_file_for_transfer(&file_path)?;

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
                        attempt += 1;
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

    // NEW: Enhanced send_file_with_target_dir using streaming
    pub async fn send_file_with_target_dir(
        &mut self,
        peer_id: Uuid,
        file_path: PathBuf,
        target_dir: Option<String>,
    ) -> Result<()> {
        info!(
            "🚀 STREAMING_SEND: Starting enhanced file transfer to {}: {:?} (target_dir: {:?})",
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

        // Create metadata with streaming support
        let metadata = if self.settings.streaming.enable_streaming_mode {
            FileMetadata::from_path_with_streaming(&file_path)?
        } else {
            // Fallback to legacy mode
            let chunk_size = self.settings.transfer.chunk_size;
            FileMetadata::from_path_with_chunk_size(&file_path, chunk_size)?
        }
        .with_target_dir(target_dir);

        let transfer_id = Uuid::new_v4();

        info!(
            "🚀 STREAMING_SEND: Created transfer {} for peer {}, file size: {} bytes, streaming: {}, chunk size: {} bytes",
            transfer_id, peer_id, metadata.size, metadata.supports_streaming, metadata.stream_chunk_size
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

        // Create progress tracker
        let progress = TransferProgress::new(
            transfer_id,
            file_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
            metadata.size,
            metadata.estimated_chunks,
        );

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
            progress,
            is_streaming: metadata.supports_streaming,
            streaming_writer: None,
        };

        // Store the transfer first
        self.active_transfers.insert(transfer_id, transfer);
        info!(
            "🚀 STREAMING_SEND: Registered outgoing transfer {} for peer {}",
            transfer_id, peer_id
        );

        // Send FileOffer through normal message channel
        if let Some(ref sender) = self.message_sender {
            let file_offer = Message::new(MessageType::FileOffer {
                transfer_id,
                metadata: metadata.clone(),
            });

            info!(
                "🚀 STREAMING_SEND: Sending FileOffer {} to message channel for peer {}",
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
                "🚀 STREAMING_SEND: FileOffer {} sent to message channel for peer {}",
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

        self.start_streaming_file_transfer(transfer_id).await?;
        Ok(())
    }

    // NEW: Enhanced streaming file transfer
    async fn start_streaming_file_transfer(&mut self, transfer_id: Uuid) -> Result<()> {
        let (file_path, peer_id, chunk_size, is_streaming, progress_interval) = {
            let transfer = self
                .active_transfers
                .get_mut(&transfer_id)
                .ok_or_else(|| FileshareError::Transfer("Transfer not found".to_string()))?;

            transfer.status = TransferStatus::Active;
            transfer.last_activity = Instant::now();

            let chunk_size = if transfer.is_streaming {
                transfer.metadata.stream_chunk_size
            } else {
                transfer.metadata.chunk_size
            };

            (
                transfer.file_path.clone(),
                transfer.peer_id,
                chunk_size,
                transfer.is_streaming,
                self.settings.streaming.progress_report_interval_ms,
            )
        };

        let message_sender = self
            .message_sender
            .clone()
            .ok_or_else(|| FileshareError::Transfer("Message sender not configured".to_string()))?;

        info!(
            "🚀 Starting {} transfer for {} with chunk size {}",
            if is_streaming { "streaming" } else { "legacy" },
            transfer_id,
            chunk_size
        );

        if is_streaming {
            // Use new streaming implementation
            tokio::spawn(async move {
                if let Err(e) = Self::send_file_chunks_streaming(
                    message_sender,
                    peer_id,
                    transfer_id,
                    file_path,
                    chunk_size,
                    progress_interval,
                )
                .await
                {
                    error!("Failed to send streaming file chunks: {}", e);
                }
            });
        } else {
            // Use legacy implementation for backward compatibility
            tokio::spawn(async move {
                if let Err(e) = Self::send_file_chunks_legacy(
                    message_sender,
                    peer_id,
                    transfer_id,
                    file_path,
                    chunk_size,
                )
                .await
                {
                    error!("Failed to send legacy file chunks: {}", e);
                }
            });
        }

        Ok(())
    }

    // NEW: Streaming implementation - NO DELAYS, MEMORY EFFICIENT
    async fn send_file_chunks_streaming(
        message_sender: MessageSender,
        peer_id: Uuid,
        transfer_id: Uuid,
        file_path: PathBuf,
        chunk_size: usize,
        progress_interval_ms: u64,
    ) -> Result<()> {
        use sha2::{Digest, Sha256};

        info!(
            "🚀 STREAMING_CHUNKS: Starting streaming file transfer for {} to peer {}",
            transfer_id, peer_id
        );

        // Create streaming file reader
        let mut reader = StreamingFileReader::new(file_path.clone(), chunk_size).await?;
        let total_size = reader.file_size();

        info!(
            "📊 STREAMING_CHUNKS: File size: {} bytes, chunk size: {} bytes",
            total_size, chunk_size
        );

        let mut hasher = Sha256::new();
        let mut chunk_count = 0u64;
        let mut bytes_sent = 0u64;
        let mut last_progress_report = Instant::now();

        // Send file in chunks using streaming
        while let Some(chunk) = reader.read_next_chunk().await? {
            // Update checksum with chunk data
            hasher.update(&chunk.data);

            info!(
                "📤 STREAMING_CHUNKS: Sending chunk {} for transfer {}: {} bytes, is_last: {}",
                chunk.index,
                transfer_id,
                chunk.data.len(),
                chunk.is_last
            );

            let chunk_data_len = chunk.data.len();
            let message = Message::new(MessageType::FileChunk { transfer_id, chunk });

            // Send through normal message channel - NO DELAYS!
            if let Err(e) = message_sender.send((peer_id, message)) {
                error!(
                    "❌ STREAMING_CHUNKS: Failed to send chunk {}: {}",
                    chunk_count, e
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

            bytes_sent += chunk_data_len as u64;
            chunk_count += 1;

            // Report progress periodically
            if last_progress_report.elapsed().as_millis() >= progress_interval_ms as u128 {
                let progress_msg = Message::new(MessageType::TransferProgress {
                    transfer_id,
                    bytes_transferred: bytes_sent,
                    chunks_completed: chunk_count,
                    transfer_rate: bytes_sent as f64 / last_progress_report.elapsed().as_secs_f64(),
                });
                let _ = message_sender.send((peer_id, progress_msg));
                last_progress_report = Instant::now();
            }
        }

        // Send completion message through normal channel
        let checksum = format!("{:x}", hasher.finalize());
        info!(
            "✅ STREAMING_CHUNKS: Transfer {} complete. Total bytes sent: {}, Total chunks: {}, Checksum: {}",
            transfer_id, bytes_sent, chunk_count, checksum
        );

        let complete_msg = Message::new(MessageType::TransferComplete {
            transfer_id,
            checksum,
        });

        if let Err(e) = message_sender.send((peer_id, complete_msg)) {
            error!(
                "❌ STREAMING_CHUNKS: Failed to send transfer complete: {}",
                e
            );
        } else {
            info!(
                "✅ STREAMING_CHUNKS: Transfer {} completed successfully",
                transfer_id
            );
        }

        Ok(())
    }

    // Legacy implementation for backward compatibility
    async fn send_file_chunks_legacy(
        message_sender: MessageSender,
        peer_id: Uuid,
        transfer_id: Uuid,
        file_path: PathBuf,
        chunk_size: usize,
    ) -> Result<()> {
        use sha2::{Digest, Sha256};

        info!(
            "🚀 LEGACY_CHUNKS: Starting legacy file transfer for {} to peer {}",
            transfer_id, peer_id
        );

        // Read the entire file into memory (legacy approach)
        let file_data = std::fs::read(&file_path).map_err(|e| {
            error!("Failed to read file {:?}: {}", file_path, e);
            FileshareError::FileOperation(format!("Failed to read file: {}", e))
        })?;

        info!(
            "📊 LEGACY_CHUNKS: File size: {} bytes, chunk size: {} bytes",
            file_data.len(),
            chunk_size
        );

        let expected_chunks = (file_data.len() + chunk_size - 1) / chunk_size;
        info!("📦 LEGACY_CHUNKS: Expected chunks: {}", expected_chunks);

        let mut hasher = Sha256::new();
        hasher.update(&file_data);

        let mut chunk_index = 0u64;
        let mut bytes_sent = 0;

        // Send file in chunks - NO DELAYS!
        while bytes_sent < file_data.len() {
            let chunk_start = bytes_sent;
            let chunk_end = std::cmp::min(bytes_sent + chunk_size, file_data.len());
            let chunk_data = file_data[chunk_start..chunk_end].to_vec();
            let is_last = chunk_end >= file_data.len();

            // Calculate CRC32 for chunk validation
            let mut crc_hasher = crc32fast::Hasher::new();
            crc_hasher.update(&chunk_data);
            let checksum = crc_hasher.finalize();

            info!(
                "📤 LEGACY_CHUNKS: Sending chunk {} for transfer {}: bytes {}-{} ({} bytes, is_last: {})",
                chunk_index, transfer_id, chunk_start, chunk_end - 1, chunk_data.len(), is_last
            );

            let chunk = TransferChunk {
                index: chunk_index,
                data: chunk_data,
                is_last,
                checksum,
                compressed_size: None,
            };

            let message = Message::new(MessageType::FileChunk { transfer_id, chunk });

            // Send through normal message channel - NO DELAYS!
            if let Err(e) = message_sender.send((peer_id, message)) {
                error!(
                    "❌ LEGACY_CHUNKS: Failed to send chunk {}: {}",
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
        }

        // Send completion message
        let checksum = format!("{:x}", hasher.finalize());
        info!(
            "✅ LEGACY_CHUNKS: Transfer {} complete. Total bytes sent: {}, Total chunks: {}, Checksum: {}",
            transfer_id, bytes_sent, chunk_index, checksum
        );

        let complete_msg = Message::new(MessageType::TransferComplete {
            transfer_id,
            checksum,
        });

        if let Err(e) = message_sender.send((peer_id, complete_msg)) {
            error!("❌ LEGACY_CHUNKS: Failed to send transfer complete: {}", e);
        } else {
            info!(
                "✅ LEGACY_CHUNKS: Transfer {} completed successfully",
                transfer_id
            );
        }

        Ok(())
    }

    pub async fn mark_outgoing_transfer_completed(&mut self, transfer_id: Uuid) -> Result<()> {
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
        }

        {
            if let Some(transfer) = self.active_transfers.get(&transfer_id) {
                if let Err(e) = self.show_transfer_notification(transfer).await {
                    warn!("Failed to show notification: {}", e);
                }
            }
        }

        Ok(())
    }

    // Enhanced monitor with streaming support
    pub async fn monitor_transfer_health(&mut self) -> Result<()> {
        let now = Instant::now();
        let mut timed_out_transfers = Vec::new();
        let mut stale_transfers = Vec::new();

        for (transfer_id, transfer) in &self.active_transfers {
            // Skip monitoring for completed transfers
            match &transfer.status {
                TransferStatus::Completed => {
                    debug!(
                        "✅ Skipping health check for completed transfer {}",
                        transfer_id
                    );
                    continue;
                }
                TransferStatus::Error(_) => {
                    debug!(
                        "⚠️ Skipping health check for errored transfer {}",
                        transfer_id
                    );
                    continue;
                }
                TransferStatus::Cancelled => {
                    debug!(
                        "🚫 Skipping health check for cancelled transfer {}",
                        transfer_id
                    );
                    continue;
                }
                _ => {}
            }

            // Use longer timeout for large files
            let timeout_seconds = if transfer.metadata.size > 1024 * 1024 * 1024 {
                TRANSFER_TIMEOUT_SECONDS * 3 // 3 hours for files > 1GB
            } else {
                TRANSFER_TIMEOUT_SECONDS
            };

            // Check for overall transfer timeout
            if now.duration_since(transfer.created_at).as_secs() > timeout_seconds {
                error!(
                    "⏰ Transfer {} timed out after {} seconds",
                    transfer_id, timeout_seconds
                );
                timed_out_transfers.push(*transfer_id);
                continue;
            }

            // Check for inactive transfers
            if now.duration_since(transfer.last_activity).as_secs() > CHUNK_TIMEOUT_SECONDS {
                match transfer.status {
                    TransferStatus::Active | TransferStatus::Pending => {
                        warn!(
                            "⚠️ Transfer {} inactive for {} seconds",
                            transfer_id, CHUNK_TIMEOUT_SECONDS
                        );
                        stale_transfers.push(*transfer_id);
                    }
                    _ => {}
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

    async fn handle_transfer_timeout(&mut self, transfer_id: Uuid) -> Result<()> {
        if let Some(transfer) = self.active_transfers.get_mut(&transfer_id) {
            transfer.status = TransferStatus::Error("Transfer timed out".to_string());

            // Notify UI about timeout
            if let Some(ref sender) = self.message_sender {
                let error_msg = Message::new(MessageType::TransferError {
                    transfer_id,
                    error: "Transfer timed out".to_string(),
                });
                let _ = sender.send((transfer.peer_id, error_msg));
            }

            self.cleanup_transfer_resources(transfer_id).await?;
        }
        Ok(())
    }

    async fn attempt_transfer_recovery(&mut self, transfer_id: Uuid) -> Result<()> {
        let recovery_data = if let Some(transfer) = self.active_transfers.get_mut(&transfer_id) {
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
                _ => {}
            }

            if matches!(transfer.direction, TransferDirection::Outgoing)
                && matches!(
                    transfer.status,
                    TransferStatus::Active | TransferStatus::Pending
                )
            {
                info!("🔄 Attempting recovery for stale transfer {}", transfer_id);

                transfer.retry_state.attempt_count += 1;
                transfer.last_activity = Instant::now();

                if transfer.retry_state.attempt_count <= MAX_RETRY_ATTEMPTS {
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
                None
            }
        } else {
            None
        };

        if let Some((file_path, peer_id, attempt_count)) = recovery_data {
            info!("🔄 Removing old transfer {} for fresh retry", transfer_id);
            self.active_transfers.remove(&transfer_id);

            let delay = self.calculate_retry_delay(attempt_count - 1);
            sleep(delay).await;

            info!("🚀 Starting fresh transfer after recovery delay");
            self.send_file_with_target_dir(peer_id, file_path, None)
                .await?;
        }

        Ok(())
    }

    async fn cleanup_transfer_resources(&mut self, transfer_id: Uuid) -> Result<()> {
        if let Some(transfer) = self.active_transfers.remove(&transfer_id) {
            info!("🧹 Cleaning up resources for transfer {}", transfer_id);

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

    pub fn cleanup_stale_transfers_enhanced(&mut self) {
        let now = Instant::now();
        let timeout = Duration::from_secs(TRANSFER_TIMEOUT_SECONDS);
        let completion_grace_period = Duration::from_secs(30);

        let initial_count = self.active_transfers.len();

        self.active_transfers.retain(|transfer_id, transfer| {
            match &transfer.status {
                TransferStatus::Completed => {
                    if now.duration_since(transfer.last_activity) > completion_grace_period {
                        info!("🧹 Cleaning up completed transfer: {}", transfer_id);
                        return false;
                    } else {
                        return true;
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
                _ => {}
            }

            // Extended timeout for large files
            let file_timeout = if transfer.metadata.size > 1024 * 1024 * 1024 {
                timeout * 3 // 3x timeout for files > 1GB
            } else {
                timeout
            };

            if now.duration_since(transfer.created_at) > file_timeout {
                warn!("🧹 Removing timed out transfer: {}", transfer_id);
                return false;
            }

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
            debug!("🧹 Enhanced cleanup removed {} transfers", removed_count);
        }
    }

    pub async fn handle_file_offer(
        &mut self,
        peer_id: Uuid,
        transfer_id: Uuid,
        metadata: FileMetadata,
    ) -> Result<()> {
        info!(
            "📥 RECEIVER: Received file offer from {}: {} ({} bytes, streaming: {}, chunk_size: {})",
            peer_id, metadata.name, metadata.size, metadata.supports_streaming, metadata.stream_chunk_size
        );

        self.cleanup_stale_transfers_enhanced();

        if self.active_transfers.contains_key(&transfer_id) {
            warn!(
                "⚠️ RECEIVER: Transfer {} already exists, ignoring duplicate offer",
                transfer_id
            );
            return Ok(());
        }

        let save_path = self.get_save_path(&metadata.name, metadata.target_dir.as_deref())?;

        // Create progress tracker
        let progress = TransferProgress::new(
            transfer_id,
            metadata.name.clone(),
            metadata.size,
            metadata.estimated_chunks,
        );

        // Create streaming writer if needed
        let streaming_writer = if metadata.supports_streaming {
            Some(
                StreamingFileWriter::new(
                    save_path.clone(),
                    metadata.size,
                    metadata.stream_chunk_size,
                )
                .await?,
            )
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
            chunks_received: vec![false; metadata.estimated_chunks as usize],
            file_handle: None,
            received_data: if metadata.supports_streaming {
                Vec::new() // Don't pre-allocate for streaming
            } else {
                vec![0u8; metadata.size as usize] // Pre-allocate for legacy
            },
            created_at: Instant::now(),
            retry_state: RetryState::default(),
            last_activity: Instant::now(),
            progress,
            is_streaming: metadata.supports_streaming,
            streaming_writer,
        };

        self.active_transfers.insert(transfer_id, transfer);
        self.accept_file_transfer(transfer_id).await?;

        info!(
            "✅ RECEIVER: File offer accepted for transfer {} (streaming: {})",
            transfer_id, metadata.supports_streaming
        );
        Ok(())
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

    pub async fn handle_file_chunk(
        &mut self,
        peer_id: Uuid,
        transfer_id: Uuid,
        chunk: TransferChunk,
    ) -> Result<()> {
        debug!(
            "📥 RECEIVER: Received chunk {} for transfer {} ({} bytes, is_last: {})",
            chunk.index,
            transfer_id,
            chunk.data.len(),
            chunk.is_last
        );

        let transfer = self.active_transfers.get(&transfer_id);
        if let Some(transfer) = transfer {
            if !matches!(transfer.direction, TransferDirection::Incoming) {
                warn!(
                    "⚠️ RECEIVER: Ignoring chunk {} for outgoing transfer {}",
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

        // Route to appropriate handler based on streaming support
        let is_streaming = self
            .active_transfers
            .get(&transfer_id)
            .unwrap()
            .is_streaming;

        if is_streaming {
            self.handle_streaming_chunk(peer_id, transfer_id, chunk)
                .await
        } else {
            self.handle_legacy_chunk(peer_id, transfer_id, chunk).await
        }
    }

    // NEW: Streaming chunk handler
    async fn handle_streaming_chunk(
        &mut self,
        _peer_id: Uuid,
        transfer_id: Uuid,
        chunk: TransferChunk,
    ) -> Result<()> {
        let is_complete = {
            let transfer = self.active_transfers.get_mut(&transfer_id).unwrap();
            transfer.last_activity = Instant::now();

            // Get streaming writer
            if let Some(ref mut writer) = transfer.streaming_writer {
                let is_complete = writer.write_chunk(chunk).await?;

                // Update transfer progress
                let (bytes_written, _, _) = writer.get_progress();
                transfer.bytes_transferred = bytes_written;
                transfer
                    .progress
                    .update(bytes_written, writer.chunks_received() as u64);

                is_complete
            } else {
                return Err(FileshareError::Transfer(
                    "No streaming writer available".to_string(),
                ));
            }
        };

        if is_complete {
            info!(
                "🎉 RECEIVER: Streaming transfer {} is complete, finalizing...",
                transfer_id
            );
            self.complete_streaming_transfer(transfer_id).await?;
        }

        Ok(())
    }

    // Legacy chunk handler for backward compatibility
    async fn handle_legacy_chunk(
        &mut self,
        peer_id: Uuid,
        transfer_id: Uuid,
        chunk: TransferChunk,
    ) -> Result<()> {
        // Use existing legacy implementation
        let is_complete = {
            let transfer = self.active_transfers.get_mut(&transfer_id).unwrap();

            if transfer.peer_id != peer_id {
                return Err(FileshareError::Transfer(
                    "Chunk from wrong peer".to_string(),
                ));
            }

            transfer.last_activity = Instant::now();

            // Legacy chunk processing
            let chunk_size = transfer.metadata.chunk_size as u64;
            let expected_offset = chunk.index * chunk_size;
            let actual_end_offset = expected_offset + chunk.data.len() as u64;
            let expected_file_size = transfer.received_data.len() as u64;

            if actual_end_offset > expected_file_size {
                return Err(FileshareError::Transfer(format!(
                    "Chunk {} too large",
                    chunk.index
                )));
            }

            let start_idx = expected_offset as usize;
            let end_idx = actual_end_offset as usize;
            transfer.received_data[start_idx..end_idx].copy_from_slice(&chunk.data);

            transfer.chunks_received[chunk.index as usize] = true;
            transfer.bytes_transferred += chunk.data.len() as u64;

            let chunks_received_count = transfer.chunks_received.iter().filter(|&&x| x).count();

            // Update progress
            transfer
                .progress
                .update(transfer.bytes_transferred, chunks_received_count as u64);

            chunk.is_last || transfer.chunks_received.iter().all(|&received| received)
        };

        if is_complete {
            self.complete_legacy_transfer(transfer_id).await?;
        }

        Ok(())
    }

    async fn complete_streaming_transfer(&mut self, transfer_id: Uuid) -> Result<()> {
        // Mark as completed first
        {
            if let Some(transfer) = self.active_transfers.get_mut(&transfer_id) {
                transfer.status = TransferStatus::Completed;
                transfer.last_activity = Instant::now();

                info!(
                    "🎉 RECEIVER: Streaming transfer {} completed: {:?}",
                    transfer_id, transfer.file_path
                );
            }
        }

        // Show notification with immutable borrow
        if let Some(transfer) = self.active_transfers.get(&transfer_id) {
            if let Err(e) = self.show_transfer_notification(transfer).await {
                warn!("Failed to show notification: {}", e);
            }
        }

        Ok(())
    }

    async fn complete_legacy_transfer(&mut self, transfer_id: Uuid) -> Result<()> {
        let (file_path, file_data, _peer_id) = {
            let transfer = self.active_transfers.get_mut(&transfer_id).unwrap();
            transfer.status = TransferStatus::Completed;
            transfer.last_activity = Instant::now();

            (
                transfer.file_path.clone(),
                transfer.received_data.clone(),
                transfer.peer_id,
            )
        };

        // Write the complete file
        std::fs::write(&file_path, &file_data)
            .map_err(|e| FileshareError::FileOperation(format!("Failed to write file: {}", e)))?;

        info!("✅ RECEIVER: Legacy transfer completed: {:?}", file_path);

        if let Some(transfer) = self.active_transfers.get(&transfer_id) {
            if let Err(e) = self.show_transfer_notification(transfer).await {
                warn!("Failed to show notification: {}", e);
            }
        }

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

    pub async fn handle_transfer_progress(
        &mut self,
        _peer_id: Uuid,
        transfer_id: Uuid,
        bytes_transferred: u64,
        chunks_completed: u64,
        transfer_rate: f64,
    ) -> Result<()> {
        if let Some(transfer) = self.active_transfers.get_mut(&transfer_id) {
            // Update progress for outgoing transfers based on peer feedback
            if matches!(transfer.direction, TransferDirection::Outgoing) {
                transfer
                    .progress
                    .update(bytes_transferred, chunks_completed);
                transfer.last_activity = Instant::now();

                debug!(
                    "📊 Updated progress for outgoing transfer {}: {:.1}%, {:.1} MB/s",
                    transfer_id,
                    transfer.progress.progress_percentage(),
                    transfer_rate / (1024.0 * 1024.0)
                );
            }
        }
        Ok(())
    }

    // NEW: Handle transfer pause
    pub async fn handle_transfer_pause(&mut self, _peer_id: Uuid, transfer_id: Uuid) -> Result<()> {
        if let Some(transfer) = self.active_transfers.get_mut(&transfer_id) {
            transfer.status = TransferStatus::Paused;
            transfer.last_activity = Instant::now();
            info!("⏸️ Transfer {} paused", transfer_id);
        }
        Ok(())
    }

    // NEW: Handle transfer resume
    pub async fn handle_transfer_resume(
        &mut self,
        _peer_id: Uuid,
        transfer_id: Uuid,
    ) -> Result<()> {
        if let Some(transfer) = self.active_transfers.get_mut(&transfer_id) {
            transfer.status = TransferStatus::Active;
            transfer.last_activity = Instant::now();
            info!("▶️ Transfer {} resumed", transfer_id);
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
        let size_str = if transfer.metadata.size < 1024 {
            format!("{} B", transfer.metadata.size)
        } else if transfer.metadata.size < 1024 * 1024 {
            format!("{:.1} KB", transfer.metadata.size as f64 / 1024.0)
        } else if transfer.metadata.size < 1024 * 1024 * 1024 {
            format!(
                "{:.1} MB",
                transfer.metadata.size as f64 / (1024.0 * 1024.0)
            )
        } else {
            format!(
                "{:.1} GB",
                transfer.metadata.size as f64 / (1024.0 * 1024.0 * 1024.0)
            )
        };

        let message = match transfer.direction {
            TransferDirection::Incoming => {
                format!(
                    "Received: {} ({})\nMode: {}",
                    transfer.metadata.name,
                    size_str,
                    if transfer.is_streaming {
                        "Streaming"
                    } else {
                        "Legacy"
                    }
                )
            }
            TransferDirection::Outgoing => {
                format!(
                    "Sent: {} ({})\nMode: {}",
                    transfer.metadata.name,
                    size_str,
                    if transfer.is_streaming {
                        "Streaming"
                    } else {
                        "Legacy"
                    }
                )
            }
        };

        notify_rust::Notification::new()
            .summary("🚀 File Transfer Complete")
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
                "Transfer {}: {:?} -> {:?} (Status: {:?}, Streaming: {}, Size: {}, Progress: {:.1}%)",
                transfer_id,
                transfer.direction,
                transfer.file_path.file_name().unwrap_or_default(),
                transfer.status,
                transfer.is_streaming,
                transfer.metadata.size,
                transfer.progress.progress_percentage()
            );
        }
        info!("=== END TRANSFERS DEBUG ===");
    }
}
