use crate::{config::Settings, network::protocol::*, FileshareError, Result};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

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
}

impl FileTransferManager {
    pub async fn new(settings: Arc<Settings>) -> Result<Self> {
        Ok(Self {
            settings,
            active_transfers: HashMap::new(),
        })
    }

    pub async fn send_file(&mut self, peer_id: Uuid, file_path: PathBuf) -> Result<()> {
        info!("Starting file transfer to {}: {:?}", peer_id, file_path);

        // Validate file exists and is readable
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

        // Create file metadata
        let metadata = FileMetadata::from_path(&file_path)?;
        let transfer_id = Uuid::new_v4();

        // Create transfer record
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

        self.active_transfers.insert(transfer_id, transfer);

        // TODO: Send file offer to peer
        // This would normally send a FileOffer message through the peer manager
        info!("File offer sent for transfer {}", transfer_id);

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

        // Determine where to save the file
        let save_path = self.get_save_path(&metadata.name)?;

        // Calculate number of chunks
        let chunk_size = self.settings.transfer.chunk_size as u64;
        let num_chunks = (metadata.size + chunk_size - 1) / chunk_size;
        let chunks_received = vec![false; num_chunks as usize];

        // Create transfer record
        let transfer = FileTransfer {
            id: transfer_id,
            peer_id,
            metadata: metadata.clone(),
            file_path: save_path,
            direction: TransferDirection::Incoming,
            status: TransferStatus::Pending,
            bytes_transferred: 0,
            chunks_received,
            file_handle: None,
        };

        self.active_transfers.insert(transfer_id, transfer);

        // Auto-accept for now (in a real app, you'd show a prompt)
        self.accept_file_transfer(transfer_id).await?;

        Ok(())
    }

    async fn accept_file_transfer(&mut self, transfer_id: Uuid) -> Result<()> {
        let transfer = self
            .active_transfers
            .get_mut(&transfer_id)
            .ok_or_else(|| FileshareError::Transfer("Transfer not found".to_string()))?;

        // Create the file
        let file = File::create(&transfer.file_path)
            .map_err(|e| FileshareError::FileOperation(format!("Failed to create file: {}", e)))?;

        transfer.file_handle = Some(file);
        transfer.status = TransferStatus::Active;

        info!(
            "Accepted file transfer {} - saving to {:?}",
            transfer_id, transfer.file_path
        );

        // TODO: Send acceptance message to peer
        Ok(())
    }

    pub async fn handle_file_chunk(
        &mut self,
        peer_id: Uuid,
        transfer_id: Uuid,
        chunk: TransferChunk,
    ) -> Result<()> {
        // Split the borrowing to avoid conflicts
        let (_file_path, chunk_size) = {
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

            (
                transfer.file_path.clone(),
                self.settings.transfer.chunk_size as u64,
            )
        };

        // Now we can safely get mutable access
        let transfer = self.active_transfers.get_mut(&transfer_id).unwrap();

        // Write chunk to file
        if let Some(ref mut file) = transfer.file_handle {
            let offset = chunk.index * chunk_size;

            file.seek(SeekFrom::Start(offset))
                .map_err(|e| FileshareError::FileOperation(format!("Seek failed: {}", e)))?;

            file.write_all(&chunk.data)
                .map_err(|e| FileshareError::FileOperation(format!("Write failed: {}", e)))?;

            file.flush()
                .map_err(|e| FileshareError::FileOperation(format!("Flush failed: {}", e)))?;

            // Update transfer progress
            if chunk.index < transfer.chunks_received.len() as u64 {
                transfer.chunks_received[chunk.index as usize] = true;
                transfer.bytes_transferred += chunk.data.len() as u64;

                debug!(
                    "Received chunk {} for transfer {} ({} bytes)",
                    chunk.index,
                    transfer_id,
                    chunk.data.len()
                );
            }

            // Check if transfer is complete
            if chunk.is_last || transfer.chunks_received.iter().all(|&received| received) {
                self.complete_file_transfer(transfer_id).await?;
            }

            // TODO: Send chunk acknowledgment
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

            // Close file handle
            if let Some(ref mut file) = transfer.file_handle {
                file.flush().map_err(|e| {
                    FileshareError::FileOperation(format!("Final flush failed: {}", e))
                })?;
            }
            transfer.file_handle = None;

            (
                transfer.file_path.clone(),
                transfer.metadata.checksum.clone(),
            )
        };

        // Verify file integrity
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
        }

        info!(
            "File transfer {} completed successfully: {:?}",
            transfer_id, file_path
        );

        // Show system notification
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
        // Handle outgoing transfer completion
        if let Some(transfer) = self.active_transfers.get_mut(&transfer_id) {
            transfer.status = TransferStatus::Completed;
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
        let downloads_dir = if let Some(ref temp_dir) = self.settings.transfer.temp_dir {
            temp_dir.clone()
        } else {
            directories::UserDirs::new()
                .and_then(|dirs| dirs.download_dir().map(|d| d.to_path_buf()))
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        };

        if !downloads_dir.exists() {
            std::fs::create_dir_all(&downloads_dir).map_err(|e| {
                FileshareError::FileOperation(format!(
                    "Failed to create downloads directory: {}",
                    e
                ))
            })?;
        }

        let mut save_path = downloads_dir.join(filename);

        // Handle filename conflicts
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
            save_path = downloads_dir.join(new_filename);
            counter += 1;
        }

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
}
