use crate::{config::Settings, network::protocol::*, FileshareError, Result};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

// Add this new type for sending messages to peers
pub type MessageSender = mpsc::UnboundedSender<(Uuid, Message)>;

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
    pub file_handle: Option<File>,
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
    active_transfers: HashMap<Uuid, FileTransfer>,
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

    pub fn set_message_sender(&mut self, sender: MessageSender) {
        self.message_sender = Some(sender);
    }

    pub async fn send_file(&mut self, peer_id: Uuid, file_path: PathBuf) -> Result<()> {
        info!(
            "🚀 SEND_FILE: Starting file transfer to {}: {:?}",
            peer_id, file_path
        );

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

        let metadata = FileMetadata::from_path(&file_path)?;
        let transfer_id = Uuid::new_v4();

        info!(
            "🚀 SEND_FILE: Created transfer {} for peer {}, file size: {} bytes",
            transfer_id, peer_id, metadata.size
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
        };

        // Store the transfer first
        self.active_transfers.insert(transfer_id, transfer);
        info!(
            "🚀 SEND_FILE: Registered outgoing transfer {} for peer {}",
            transfer_id, peer_id
        );

        if let Some(ref sender) = self.message_sender {
            let file_offer = Message::new(MessageType::FileOffer {
                transfer_id,
                metadata: metadata.clone(),
            });

            info!(
                "🚀 SEND_FILE: About to send FileOffer {} to peer {}",
                transfer_id, peer_id
            );

            if let Err(e) = sender.send((peer_id, file_offer)) {
                error!("Failed to send file offer: {}", e);
                // Remove the failed transfer
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
            // Remove the transfer if we can't send
            self.active_transfers.remove(&transfer_id);
            return Err(FileshareError::Transfer(
                "Message sender not configured".to_string(),
            ));
        }

        Ok(())
    }

    pub async fn handle_file_offer_response(
        &mut self,
        peer_id: Uuid,
        transfer_id: Uuid,
        accepted: bool,
        reason: Option<String>,
    ) -> Result<()> {
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

        info!("File offer {} accepted by peer {}", transfer_id, peer_id);
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
            (
                transfer.file_path.clone(),
                transfer.peer_id,
                self.settings.transfer.chunk_size,
            )
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
            "Starting to send file chunks for transfer {} to peer {}",
            transfer_id, peer_id
        );
        info!("Reading file: {:?}", file_path);

        // Verify file exists and is readable
        if !file_path.exists() {
            let error_msg = Message::new(MessageType::TransferError {
                transfer_id,
                error: "Source file does not exist".to_string(),
            });
            let _ = message_sender.send((peer_id, error_msg));
            return Err(FileshareError::FileOperation(
                "Source file does not exist".to_string(),
            ));
        }

        let mut file = std::fs::File::open(&file_path).map_err(|e| {
            error!("Failed to open file {:?}: {}", file_path, e);
            FileshareError::FileOperation(format!("Failed to open file: {}", e))
        })?;

        // Get file size for validation
        let file_metadata = file.metadata().map_err(|e| {
            FileshareError::FileOperation(format!("Failed to get file metadata: {}", e))
        })?;

        info!("File size: {} bytes", file_metadata.len());

        let mut buffer = vec![0u8; chunk_size];
        let mut chunk_index = 0u64;
        let mut hasher = Sha256::new();
        let mut total_bytes_sent = 0u64;

        loop {
            match file.read(&mut buffer) {
                Ok(0) => {
                    // End of file
                    let checksum = format!("{:x}", hasher.finalize());
                    info!(
                        "File read complete. Total bytes sent: {}, Checksum: {}",
                        total_bytes_sent, checksum
                    );

                    let complete_msg = Message::new(MessageType::TransferComplete {
                        transfer_id,
                        checksum,
                    });

                    if let Err(e) = message_sender.send((peer_id, complete_msg)) {
                        error!("Failed to send transfer complete: {}", e);
                    } else {
                        info!("File transfer {} completed successfully", transfer_id);
                    }
                    break;
                }
                Ok(bytes_read) => {
                    let chunk_data = buffer[..bytes_read].to_vec();
                    let is_last = bytes_read < chunk_size;
                    total_bytes_sent += bytes_read as u64;

                    // DEBUG: Log what we're actually reading
                    info!(
                        "Read chunk {}: {} bytes (total: {}/{})",
                        chunk_index,
                        bytes_read,
                        total_bytes_sent,
                        file_metadata.len()
                    );

                    // DEBUG: Show the actual content being read
                    let content_preview = if chunk_data.len() <= 100 {
                        String::from_utf8_lossy(&chunk_data).to_string()
                    } else {
                        format!("{}...", String::from_utf8_lossy(&chunk_data[..100]))
                    };
                    info!(
                        "Chunk {} content preview: '{}'",
                        chunk_index, content_preview
                    );

                    // DEBUG: Show raw bytes
                    info!(
                        "Chunk {} raw bytes: {:?}",
                        chunk_index,
                        &chunk_data[..std::cmp::min(20, chunk_data.len())]
                    );

                    hasher.update(&chunk_data);

                    let chunk = TransferChunk {
                        index: chunk_index,
                        data: chunk_data,
                        is_last,
                    };

                    let message = Message::new(MessageType::FileChunk { transfer_id, chunk });

                    if let Err(e) = message_sender.send((peer_id, message)) {
                        error!("Failed to send chunk {}: {}", chunk_index, e);

                        let error_msg = Message::new(MessageType::TransferError {
                            transfer_id,
                            error: format!("Failed to send chunk: {}", e),
                        });
                        let _ = message_sender.send((peer_id, error_msg));
                        break;
                    }

                    info!(
                        "Sent chunk {} for transfer {} ({} bytes)",
                        chunk_index, transfer_id, bytes_read
                    );
                    chunk_index += 1;

                    // Small delay to prevent overwhelming the network
                    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
                }
                Err(e) => {
                    error!("Error reading file: {}", e);
                    let error_msg = Message::new(MessageType::TransferError {
                        transfer_id,
                        error: format!("Read error: {}", e),
                    });

                    let _ = message_sender.send((peer_id, error_msg));
                    break;
                }
            }
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
            "Received file offer from {}: {} ({} bytes)",
            peer_id, metadata.name, metadata.size
        );

        let save_path = self.get_save_path(&metadata.name)?;
        let chunk_size = self.settings.transfer.chunk_size as u64;
        let num_chunks = if metadata.size == 0 {
            1
        } else {
            (metadata.size + chunk_size - 1) / chunk_size
        };
        let chunks_received = vec![false; num_chunks as usize];

        let transfer = FileTransfer {
            id: transfer_id,
            peer_id,
            metadata: metadata.clone(),
            file_path: save_path.clone(),
            direction: TransferDirection::Incoming,
            status: TransferStatus::Pending,
            bytes_transferred: 0,
            chunks_received,
            file_handle: None,
        };

        self.active_transfers.insert(transfer_id, transfer);

        if let Some(ref sender) = self.message_sender {
            let response = Message::new(MessageType::FileOfferResponse {
                transfer_id,
                accepted: true,
                reason: None,
            });

            if let Err(e) = sender.send((peer_id, response)) {
                error!("Failed to send file offer response: {}", e);
                return Err(FileshareError::Transfer(format!(
                    "Failed to send response: {}",
                    e
                )));
            }

            info!("Sent file offer acceptance for transfer {}", transfer_id);
        }

        self.accept_file_transfer(transfer_id).await?;
        Ok(())
    }

    async fn accept_file_transfer(&mut self, transfer_id: Uuid) -> Result<()> {
        let (file_path, file_size) = {
            let transfer = self
                .active_transfers
                .get(&transfer_id)
                .ok_or_else(|| FileshareError::Transfer("Transfer not found".to_string()))?;
            (transfer.file_path.clone(), transfer.metadata.size)
        };

        info!(
            "Creating file for transfer {} at {:?} with size {} bytes",
            transfer_id, file_path, file_size
        );

        // Create the file with proper permissions and pre-allocate space
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&file_path)
            .map_err(|e| {
                error!("Failed to create file {:?}: {}", file_path, e);
                FileshareError::FileOperation(format!("Failed to create file: {}", e))
            })?;

        // Pre-allocate the file to the expected size if it's not empty
        if file_size > 0 {
            if let Err(e) = file.set_len(file_size) {
                warn!("Failed to pre-allocate file space: {}", e);
                // Continue anyway, but log the warning
            } else {
                info!("Successfully pre-allocated file to {} bytes", file_size);
            }
        }

        // Update transfer with file handle
        let transfer = self.active_transfers.get_mut(&transfer_id).unwrap();
        transfer.file_handle = Some(file);
        transfer.status = TransferStatus::Active;

        info!(
            "Accepted file transfer {} - saving to {:?}",
            transfer_id, file_path
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
            "Received chunk {} for transfer {} ({} bytes, is_last: {})",
            chunk.index,
            transfer_id,
            chunk.data.len(),
            chunk.is_last
        );

        // DEBUG: Show what we're receiving
        let content_preview = if chunk.data.len() <= 100 {
            String::from_utf8_lossy(&chunk.data).to_string()
        } else {
            format!("{}...", String::from_utf8_lossy(&chunk.data[..100]))
        };
        info!(
            "Received chunk {} content preview: '{}'",
            chunk.index, content_preview
        );

        // DEBUG: Show raw bytes
        info!(
            "Received chunk {} raw bytes: {:?}",
            chunk.index,
            &chunk.data[..std::cmp::min(20, chunk.data.len())]
        );

        // Extract necessary data without keeping mutable borrow
        let (chunk_size, should_send_ack) = {
            let transfer = self
                .active_transfers
                .get(&transfer_id)
                .ok_or_else(|| FileshareError::Transfer("Transfer not found".to_string()))?;

            if transfer.peer_id != peer_id {
                return Err(FileshareError::Transfer(
                    "Chunk from wrong peer".to_string(),
                ));
            }

            if !matches!(transfer.status, TransferStatus::Active) {
                return Err(FileshareError::Transfer("Transfer not active".to_string()));
            }

            (self.settings.transfer.chunk_size as u64, true)
        };

        // Now get mutable access for writing
        let is_complete = {
            let transfer = self.active_transfers.get_mut(&transfer_id).unwrap();

            if let Some(ref mut file) = transfer.file_handle {
                let offset = chunk.index * chunk_size;

                info!(
                    "Writing chunk {} at offset {} for transfer {}",
                    chunk.index, offset, transfer_id
                );

                // Seek to the correct position
                file.seek(SeekFrom::Start(offset)).map_err(|e| {
                    error!("Seek failed for chunk {}: {}", chunk.index, e);
                    FileshareError::FileOperation(format!("Seek failed: {}", e))
                })?;

                // Write the chunk data
                file.write_all(&chunk.data).map_err(|e| {
                    error!("Write failed for chunk {}: {}", chunk.index, e);
                    FileshareError::FileOperation(format!("Write failed: {}", e))
                })?;

                // CRITICAL: Flush immediately after each write
                file.flush().map_err(|e| {
                    error!("Flush failed for chunk {}: {}", chunk.index, e);
                    FileshareError::FileOperation(format!("Flush failed: {}", e))
                })?;

                info!(
                    "Successfully wrote and flushed chunk {} for transfer {} ({} bytes at offset {})",
                    chunk.index, transfer_id, chunk.data.len(), offset
                );

                // DEBUG: Verify what was written by reading it back
                let current_pos = file.stream_position().map_err(|e| {
                    error!("Failed to get current position: {}", e);
                    FileshareError::FileOperation(format!("Position check failed: {}", e))
                })?;

                file.seek(SeekFrom::Start(offset)).map_err(|e| {
                    error!("Seek back failed for verification: {}", e);
                    FileshareError::FileOperation(format!("Seek back failed: {}", e))
                })?;

                let mut verify_buffer = vec![0u8; chunk.data.len()];
                file.read_exact(&mut verify_buffer).map_err(|e| {
                    error!("Read back failed for verification: {}", e);
                    FileshareError::FileOperation(format!("Read back failed: {}", e))
                })?;

                let verify_preview = String::from_utf8_lossy(&verify_buffer).to_string();
                info!(
                    "Verification: chunk {} read back as: '{}'",
                    chunk.index, verify_preview
                );
                info!(
                    "Verification: raw bytes: {:?}",
                    &verify_buffer[..std::cmp::min(20, verify_buffer.len())]
                );

                // Restore file position to end of written data
                file.seek(SeekFrom::Start(current_pos)).map_err(|e| {
                    error!("Failed to restore position: {}", e);
                    FileshareError::FileOperation(format!("Position restore failed: {}", e))
                })?;

                // Mark chunk as received
                if chunk.index < transfer.chunks_received.len() as u64 {
                    transfer.chunks_received[chunk.index as usize] = true;
                    transfer.bytes_transferred += chunk.data.len() as u64;

                    let chunks_received_count =
                        transfer.chunks_received.iter().filter(|&&x| x).count();

                    info!(
                        "Chunk {} marked as received for transfer {} ({}/{} chunks, {} bytes total)",
                        chunk.index,
                        transfer_id,
                        chunks_received_count,
                        transfer.chunks_received.len(),
                        transfer.bytes_transferred
                    );
                }

                // Check if transfer is complete
                chunk.is_last || transfer.chunks_received.iter().all(|&received| received)
            } else {
                error!("No file handle for transfer {}", transfer_id);
                false
            }
        };

        if is_complete {
            info!("Transfer {} is complete, finalizing...", transfer_id);
            self.complete_file_transfer(transfer_id).await?;
        } else if should_send_ack {
            // Send acknowledgment
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
        let (file_path, expected_checksum) = {
            let transfer = self
                .active_transfers
                .get_mut(&transfer_id)
                .ok_or_else(|| FileshareError::Transfer("Transfer not found".to_string()))?;

            transfer.status = TransferStatus::Completed;

            // CRITICAL: Final flush and sync
            if let Some(ref mut file) = transfer.file_handle {
                file.flush().map_err(|e| {
                    error!("Final flush failed for transfer {}: {}", transfer_id, e);
                    FileshareError::FileOperation(format!("Final flush failed: {}", e))
                })?;

                // Force sync to disk
                file.sync_all().map_err(|e| {
                    error!("Sync failed for transfer {}: {}", transfer_id, e);
                    FileshareError::FileOperation(format!("Sync failed: {}", e))
                })?;

                info!("File flushed and synced for transfer {}", transfer_id);
            }
            transfer.file_handle = None;

            (
                transfer.file_path.clone(),
                transfer.metadata.checksum.clone(),
            )
        };

        // Verify file exists and has correct size
        if file_path.exists() {
            let actual_size = std::fs::metadata(&file_path).map(|m| m.len()).unwrap_or(0);
            info!(
                "Transfer {} completed: file exists with size {} bytes at {:?}",
                transfer_id, actual_size, file_path
            );
        } else {
            error!(
                "Transfer {} completed but file doesn't exist: {:?}",
                transfer_id, file_path
            );
        }

        // Verify checksum if provided
        if !expected_checksum.is_empty() {
            let calculated_checksum = self.calculate_file_checksum(&file_path)?;
            if calculated_checksum != expected_checksum {
                error!(
                    "Checksum mismatch for transfer {}: expected {}, got {}",
                    transfer_id, expected_checksum, calculated_checksum
                );

                if let Some(transfer) = self.active_transfers.get_mut(&transfer_id) {
                    transfer.status = TransferStatus::Error("Checksum mismatch".to_string());
                }
                return Err(FileshareError::Transfer(
                    "File integrity check failed".to_string(),
                ));
            } else {
                info!("Checksum verified for transfer {}", transfer_id);
            }
        }

        info!(
            "File transfer {} completed successfully: {:?}",
            transfer_id, file_path
        );

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
        info!("Transfer {} completed by peer", transfer_id);
        let transfer = self.active_transfers.get_mut(&transfer_id);
        if let Some(transfer) = transfer {
            transfer.status = TransferStatus::Completed;
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
        error!("Transfer {} failed: {}", transfer_id, error);
        if let Some(transfer) = self.active_transfers.get_mut(&transfer_id) {
            transfer.status = TransferStatus::Error(error);
        }
        Ok(())
    }

    fn get_save_path(&self, filename: &str) -> Result<PathBuf> {
        let save_dir = if let Some(ref temp_dir) = self.settings.transfer.temp_dir {
            temp_dir.clone()
        } else {
            // Use Downloads directory by default
            directories::UserDirs::new()
                .and_then(|dirs| dirs.download_dir().map(|d| d.to_path_buf()))
                .unwrap_or_else(|| {
                    // Fallback to Documents if Downloads doesn't exist
                    directories::UserDirs::new()
                        .and_then(|dirs| dirs.document_dir().map(|d| d.to_path_buf()))
                        .unwrap_or_else(|| PathBuf::from("."))
                })
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

    fn calculate_file_checksum(&self, file_path: &PathBuf) -> Result<String> {
        use sha2::{Digest, Sha256};

        let mut file = File::open(file_path).map_err(|e| {
            FileshareError::FileOperation(format!("Failed to open file for checksum: {}", e))
        })?;

        let mut hasher = Sha256::new();
        let mut buffer = [0; 8192];

        loop {
            let bytes_read = file.read(&mut buffer).map_err(|e| {
                FileshareError::FileOperation(format!("Failed to read file for checksum: {}", e))
            })?;

            if bytes_read == 0 {
                break;
            }

            hasher.update(&buffer[..bytes_read]);
        }

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
                "Transfer {}: {:?} -> {:?} (Status: {:?})",
                transfer_id,
                transfer.direction,
                transfer.file_path.file_name().unwrap_or_default(),
                transfer.status
            );
        }
        info!("=== END TRANSFERS DEBUG ===");
    }
}
