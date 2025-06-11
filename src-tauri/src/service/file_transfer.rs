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
    pub total_chunks: u64,
    pub chunks_received: Vec<bool>,
    pub file_handle: Option<File>,
    pub received_chunks: HashMap<u64, Vec<u8>>, // FIXED: Use HashMap to handle out-of-order chunks
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

        let metadata = FileMetadata::from_path(&file_path)?.with_target_dir(target_dir);
        let transfer_id = Uuid::new_v4();

        info!(
            "🚀 SEND_FILE: Created transfer {} for peer {}, file size: {} bytes",
            transfer_id, peer_id, metadata.size
        );

        // Calculate total chunks
        let chunk_size = self.settings.transfer.chunk_size as u64;
        let total_chunks = (metadata.size + chunk_size - 1) / chunk_size;

        // Create and store the transfer BEFORE sending the offer
        let transfer = FileTransfer {
            id: transfer_id,
            peer_id,
            metadata: metadata.clone(),
            file_path: file_path.clone(),
            direction: TransferDirection::Outgoing,
            status: TransferStatus::Pending,
            bytes_transferred: 0,
            total_chunks,
            chunks_received: vec![false; total_chunks as usize],
            file_handle: None,
            received_chunks: HashMap::new(),
        };

        // Store the transfer first
        self.active_transfers.insert(transfer_id, transfer);
        info!(
            "🚀 SEND_FILE: Registered outgoing transfer {} for peer {} ({} chunks)",
            transfer_id, peer_id, total_chunks
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

    pub fn set_message_sender(&mut self, sender: MessageSender) {
        self.message_sender = Some(sender);
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

        // Read the entire file into memory first
        let file_data = std::fs::read(&file_path).map_err(|e| {
            error!("Failed to read file {:?}: {}", file_path, e);
            FileshareError::FileOperation(format!("Failed to read file: {}", e))
        })?;

        info!("File size: {} bytes", file_data.len());
        info!(
            "First 50 bytes: {:?}",
            String::from_utf8_lossy(&file_data[..std::cmp::min(50, file_data.len())])
        );

        let mut hasher = Sha256::new();
        hasher.update(&file_data);

        let mut chunk_index = 0u64;
        let mut bytes_sent = 0;
        let total_chunks = (file_data.len() + chunk_size - 1) / chunk_size;

        info!(
            "Sending {} chunks of max {} bytes each",
            total_chunks, chunk_size
        );

        // Send file in chunks
        while bytes_sent < file_data.len() {
            let chunk_start = bytes_sent;
            let chunk_end = std::cmp::min(bytes_sent + chunk_size, file_data.len());
            let chunk_data = file_data[chunk_start..chunk_end].to_vec();
            let is_last = chunk_end >= file_data.len();

            info!(
                "Sending chunk {}/{}: bytes {}-{} ({} bytes, is_last: {})",
                chunk_index + 1,
                total_chunks,
                chunk_start,
                chunk_end - 1,
                chunk_data.len(),
                is_last
            );

            // Verify chunk content
            let content_preview =
                String::from_utf8_lossy(&chunk_data[..std::cmp::min(20, chunk_data.len())]);
            info!(
                "Chunk {} content preview: '{}'",
                chunk_index, content_preview
            );

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
                return Err(FileshareError::Transfer(format!(
                    "Failed to send chunk: {}",
                    e
                )));
            }

            bytes_sent = chunk_end;
            chunk_index += 1;

            // Small delay to prevent overwhelming the network
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }

        // Send completion message
        let checksum = format!("{:x}", hasher.finalize());
        info!(
            "File transfer complete. Total bytes sent: {}, Total chunks: {}, Checksum: {}",
            bytes_sent, chunk_index, checksum
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

        // Use target directory from metadata
        let save_path = self.get_save_path(&metadata.name, metadata.target_dir.as_deref())?;

        // Calculate total chunks for this file
        let chunk_size = self.settings.transfer.chunk_size as u64;
        let total_chunks = if metadata.size == 0 {
            1
        } else {
            (metadata.size + chunk_size - 1) / chunk_size
        };

        info!(
            "Expecting {} chunks for file {} (size: {} bytes, chunk_size: {})",
            total_chunks, metadata.name, metadata.size, chunk_size
        );

        let transfer = FileTransfer {
            id: transfer_id,
            peer_id,
            metadata: metadata.clone(),
            file_path: save_path.clone(),
            direction: TransferDirection::Incoming,
            status: TransferStatus::Pending,
            bytes_transferred: 0,
            total_chunks,
            chunks_received: vec![false; total_chunks as usize],
            file_handle: None,
            received_chunks: HashMap::new(), // Use HashMap for out-of-order chunks
        };

        self.active_transfers.insert(transfer_id, transfer);
        self.accept_file_transfer(transfer_id).await?;

        info!("File offer accepted for transfer {}", transfer_id);
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

        info!(
            "Accepted file transfer {} - will save to {:?}",
            transfer_id, transfer.file_path
        );

        Ok(())
    }

    // COMPLETELY REWRITTEN: Handle file chunks with proper ordering and assembly
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

        // Check if transfer exists and is incoming
        let transfer = self.active_transfers.get(&transfer_id);
        if let Some(transfer) = transfer {
            // Only process chunks for INCOMING transfers
            if !matches!(transfer.direction, TransferDirection::Incoming) {
                warn!(
                    "Ignoring chunk {} for outgoing transfer {} - chunks should only be processed for incoming transfers",
                    chunk.index, transfer_id
                );
                return Ok(());
            }
        } else {
            warn!("Received chunk for unknown transfer {}", transfer_id);
            return Ok(());
        }

        // Verify chunk content
        let content_preview =
            String::from_utf8_lossy(&chunk.data[..std::cmp::min(20, chunk.data.len())]);
        info!(
            "Received chunk {} content preview: '{}'",
            chunk.index, content_preview
        );

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

            // Validate chunk index
            if chunk.index >= transfer.total_chunks {
                error!(
                    "Chunk index {} exceeds total chunks {} for transfer {}",
                    chunk.index, transfer.total_chunks, transfer_id
                );
                return Err(FileshareError::Transfer("Invalid chunk index".to_string()));
            }

            // Store chunk data in HashMap (handles out-of-order delivery)
            transfer.received_chunks.insert(chunk.index, chunk.data);

            // Mark chunk as received
            if (chunk.index as usize) < transfer.chunks_received.len() {
                transfer.chunks_received[chunk.index as usize] = true;

                let chunks_received_count = transfer.chunks_received.iter().filter(|&&x| x).count();
                info!(
                    "Chunk {} stored for transfer {} ({}/{} chunks received)",
                    chunk.index, transfer_id, chunks_received_count, transfer.total_chunks
                );
            }

            // Check if transfer is complete (either is_last flag OR all chunks received)
            let all_chunks_received = transfer.chunks_received.iter().all(|&received| received);
            chunk.is_last || all_chunks_received
        };

        if is_complete {
            info!(
                "Transfer {} is complete, assembling and writing file...",
                transfer_id
            );
            self.complete_file_transfer(transfer_id).await?;
        } else {
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

    // COMPLETELY REWRITTEN: Assemble chunks in correct order and write file
    async fn complete_file_transfer(&mut self, transfer_id: Uuid) -> Result<()> {
        let (file_path, expected_checksum, metadata, received_chunks, total_chunks) = {
            let transfer = self
                .active_transfers
                .get_mut(&transfer_id)
                .ok_or_else(|| FileshareError::Transfer("Transfer not found".to_string()))?;

            transfer.status = TransferStatus::Completed;

            (
                transfer.file_path.clone(),
                transfer.metadata.checksum.clone(),
                transfer.metadata.clone(),
                transfer.received_chunks.clone(),
                transfer.total_chunks,
            )
        };

        info!(
            "Assembling file from {} chunks for transfer {}",
            total_chunks, transfer_id
        );

        // Assemble chunks in correct order
        let mut file_data = Vec::new();
        let mut expected_size = 0;

        for chunk_index in 0..total_chunks {
            if let Some(chunk_data) = received_chunks.get(&chunk_index) {
                info!(
                    "Assembling chunk {} ({} bytes)",
                    chunk_index,
                    chunk_data.len()
                );
                file_data.extend_from_slice(chunk_data);
                expected_size += chunk_data.len();
            } else {
                error!("Missing chunk {} for transfer {}", chunk_index, transfer_id);
                return Err(FileshareError::Transfer(format!(
                    "Missing chunk {} - transfer incomplete",
                    chunk_index
                )));
            }
        }

        info!("Assembled file: {} bytes total", file_data.len());

        // Verify assembled size matches expected
        if file_data.len() != metadata.size as usize {
            error!(
                "Assembled file size {} doesn't match expected size {} for transfer {}",
                file_data.len(),
                metadata.size,
                transfer_id
            );
            return Err(FileshareError::Transfer(
                "File size mismatch - transfer corrupted".to_string(),
            ));
        }

        // Show content preview
        let content_preview =
            String::from_utf8_lossy(&file_data[..std::cmp::min(100, file_data.len())]);
        info!("Assembled file content preview: '{}'", content_preview);

        info!("Writing complete file to: {:?}", file_path);

        // Write the complete file data
        std::fs::write(&file_path, &file_data).map_err(|e| {
            error!("Failed to write file {:?}: {}", file_path, e);
            FileshareError::FileOperation(format!("Failed to write file: {}", e))
        })?;

        info!("File written successfully to: {:?}", file_path);

        // Verify the file was written correctly
        let written_data = std::fs::read(&file_path).map_err(|e| {
            error!("Failed to read back written file: {}", e);
            FileshareError::FileOperation(format!("Failed to verify written file: {}", e))
        })?;

        if written_data.len() != file_data.len() {
            error!(
                "Written file size mismatch! Expected: {}, Got: {}",
                file_data.len(),
                written_data.len()
            );
            return Err(FileshareError::Transfer(
                "File write verification failed - size mismatch".to_string(),
            ));
        }

        if written_data != file_data {
            error!("Written file content mismatch!");
            return Err(FileshareError::Transfer(
                "File write verification failed - content mismatch".to_string(),
            ));
        }

        info!("File write verification successful!");

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
            "File transfer {} completed successfully: {:?} ({} bytes)",
            transfer_id,
            file_path,
            file_data.len()
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
                let chunks_received = transfer.chunks_received.iter().filter(|&&x| x).count();
                chunks_received as f32 / transfer.total_chunks as f32
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
