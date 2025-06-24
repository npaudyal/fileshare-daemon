use crate::quic::stream_manager::StreamManager;
use crate::service::streaming::{StreamingFileReader, calculate_adaptive_chunk_size};
use crate::network::protocol::{Message, MessageType, FileMetadata};
use crate::{FileshareError, Result};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{RwLock, Semaphore};
use tracing::{debug, error, info};
use uuid::Uuid;

const MAX_PARALLEL_STREAMS: usize = 16; // Maximum parallel streams for file transfer
const CHUNK_BUFFER_SIZE: usize = 64; // Number of chunks to buffer ahead
const MAX_IN_FLIGHT_CHUNKS: usize = 32; // Maximum chunks in flight at once

pub struct QuicFileTransfer {
    stream_manager: Arc<StreamManager>,
    peer_id: Uuid,
    transfer_id: Uuid,
}

impl QuicFileTransfer {
    pub fn new(stream_manager: Arc<StreamManager>, peer_id: Uuid, transfer_id: Uuid) -> Self {
        Self {
            stream_manager,
            peer_id,
            transfer_id,
        }
    }
    
    pub async fn send_file_parallel(
        &self,
        file_path: PathBuf,
        metadata: FileMetadata,
    ) -> Result<()> {
        let start_time = Instant::now();
        let chunk_size = metadata.chunk_size;
        let total_chunks = metadata.total_chunks;
        
        info!(
            "Starting parallel QUIC file transfer: {} ({} bytes, {} chunks of {} bytes)",
            metadata.name, metadata.size, total_chunks, chunk_size
        );
        
        // Create streaming file reader
        let mut reader = StreamingFileReader::new(
            &file_path,
            chunk_size,
            metadata.compression,
        ).await?;
        
        // File offer was already sent by FileTransferManager, so we can start sending chunks directly
        // No need to send duplicate FileOffer or wait
        
        // Create chunk sender
        let sender = ParallelChunkSender::new(
            self.stream_manager.clone(),
            self.transfer_id,
            MAX_PARALLEL_STREAMS,
        );
        
        // Start parallel chunk sending
        let mut chunks_sent = 0u64;
        let mut bytes_sent = 0u64;
        let mut chunk_buffer = Vec::with_capacity(CHUNK_BUFFER_SIZE);
        
        loop {
            // Fill buffer with chunks
            while chunk_buffer.len() < CHUNK_BUFFER_SIZE && chunks_sent < total_chunks {
                match reader.read_next_chunk().await? {
                    Some((data, is_last)) => {
                        let chunk_index = chunks_sent;
                        chunk_buffer.push((chunk_index, data, is_last));
                        chunks_sent += 1;
                        
                        if is_last {
                            break;
                        }
                    }
                    None => break,
                }
            }
            
            if chunk_buffer.is_empty() {
                break;
            }
            
            // Send chunks in parallel
            let batch_size = chunk_buffer.len();
            let batch_bytes: usize = chunk_buffer.iter().map(|(_, data, _)| data.len()).sum();
            
            debug!("Sending batch of {} chunks ({} bytes) in parallel", batch_size, batch_bytes);
            
            let batch_start = Instant::now();
            sender.send_chunks_batch(chunk_buffer.drain(..).collect()).await?;
            let batch_duration = batch_start.elapsed();
            
            bytes_sent += batch_bytes as u64;
            
            // Calculate and log speed
            let speed_mbps = (batch_bytes as f64 * 8.0) / (batch_duration.as_secs_f64() * 1_000_000.0);
            info!(
                "Sent {} chunks in {:.2}s ({:.2} Mbps) - Progress: {}/{}",
                batch_size,
                batch_duration.as_secs_f64(),
                speed_mbps,
                chunks_sent,
                total_chunks
            );
        }
        
        // Get final checksum
        let checksum = reader.get_checksum();
        
        // Send transfer complete message
        let complete_message = Message::new(MessageType::TransferComplete {
            transfer_id: self.transfer_id,
            checksum,
        });
        self.stream_manager.send_control_message(complete_message).await?;
        
        let total_duration = start_time.elapsed();
        let average_speed_mbps = (bytes_sent as f64 * 8.0) / (total_duration.as_secs_f64() * 1_000_000.0);
        
        info!(
            "✅ QUIC file transfer completed: {} chunks, {} bytes in {:.2}s ({:.2} Mbps average)",
            chunks_sent,
            bytes_sent,
            total_duration.as_secs_f64(),
            average_speed_mbps
        );
        
        Ok(())
    }
}

struct ParallelChunkSender {
    stream_manager: Arc<StreamManager>,
    transfer_id: Uuid,
    semaphore: Arc<Semaphore>,
}

impl ParallelChunkSender {
    fn new(
        stream_manager: Arc<StreamManager>,
        transfer_id: Uuid,
        max_parallel: usize,
    ) -> Self {
        Self {
            stream_manager,
            transfer_id,
            semaphore: Arc::new(Semaphore::new(max_parallel)),
        }
    }
    
    async fn send_chunks_batch(&self, chunks: Vec<(u64, Vec<u8>, bool)>) -> Result<()> {
        let chunk_count = chunks.len();
        
        // Send chunks in parallel using QUIC streams
        self.stream_manager.send_chunks_parallel(
            self.transfer_id,
            chunks,
            MAX_PARALLEL_STREAMS,
        ).await?;
        
        debug!("Successfully sent {} chunks in parallel", chunk_count);
        
        Ok(())
    }
}

pub struct ParallelTransferManager {
    active_transfers: Arc<RwLock<HashMap<Uuid, TransferState>>>,
    max_concurrent_transfers: usize,
}

struct TransferState {
    transfer_id: Uuid,
    peer_id: Uuid,
    file_path: PathBuf,
    metadata: FileMetadata,
    progress: Arc<RwLock<TransferProgress>>,
    start_time: Instant,
}

struct TransferProgress {
    chunks_sent: u64,
    bytes_sent: u64,
    total_chunks: u64,
    total_bytes: u64,
    last_update: Instant,
}

impl ParallelTransferManager {
    pub fn new(max_concurrent_transfers: usize) -> Self {
        Self {
            active_transfers: Arc::new(RwLock::new(HashMap::new())),
            max_concurrent_transfers,
        }
    }
    
    pub async fn start_transfer(
        &self,
        stream_manager: Arc<StreamManager>,
        peer_id: Uuid,
        file_path: PathBuf,
    ) -> Result<Uuid> {
        let transfer_id = Uuid::new_v4();
        
        // Calculate chunk size based on file size
        let file_size = tokio::fs::metadata(&file_path).await?.len();
        let chunk_size = calculate_adaptive_chunk_size(file_size);
        
        // Create file metadata
        let metadata = FileMetadata::from_path_with_chunk_size(&file_path, chunk_size)?;
        
        // Create transfer state
        let state = TransferState {
            transfer_id,
            peer_id,
            file_path: file_path.clone(),
            metadata: metadata.clone(),
            progress: Arc::new(RwLock::new(TransferProgress {
                chunks_sent: 0,
                bytes_sent: 0,
                total_chunks: metadata.total_chunks,
                total_bytes: metadata.size,
                last_update: Instant::now(),
            })),
            start_time: Instant::now(),
        };
        
        // Store transfer state
        let mut transfers = self.active_transfers.write().await;
        transfers.insert(transfer_id, state);
        drop(transfers);
        
        // Start the transfer in the background
        let transfer = QuicFileTransfer::new(stream_manager, peer_id, transfer_id);
        let transfers_ref = self.active_transfers.clone();
        
        tokio::spawn(async move {
            match transfer.send_file_parallel(file_path, metadata).await {
                Ok(()) => {
                    info!("Transfer {} completed successfully", transfer_id);
                }
                Err(e) => {
                    error!("Transfer {} failed: {}", transfer_id, e);
                }
            }
            
            // Remove from active transfers
            let mut transfers = transfers_ref.write().await;
            transfers.remove(&transfer_id);
        });
        
        Ok(transfer_id)
    }
    
    pub async fn get_transfer_progress(&self, transfer_id: Uuid) -> Option<(f32, u64)> {
        let transfers = self.active_transfers.read().await;
        
        if let Some(state) = transfers.get(&transfer_id) {
            let progress = state.progress.read().await;
            
            let percentage = if progress.total_chunks > 0 {
                (progress.chunks_sent as f32 / progress.total_chunks as f32) * 100.0
            } else {
                0.0
            };
            
            let elapsed = state.start_time.elapsed();
            let speed_bps = if elapsed.as_secs() > 0 {
                progress.bytes_sent / elapsed.as_secs()
            } else {
                0
            };
            
            Some((percentage, speed_bps))
        } else {
            None
        }
    }
    
    pub async fn cancel_transfer(&self, transfer_id: Uuid) -> Result<()> {
        let mut transfers = self.active_transfers.write().await;
        
        if transfers.remove(&transfer_id).is_some() {
            info!("Transfer {} cancelled", transfer_id);
            Ok(())
        } else {
            Err(FileshareError::Transfer(format!(
                "Transfer {} not found",
                transfer_id
            )))
        }
    }
    
    pub async fn get_active_transfers(&self) -> Vec<Uuid> {
        let transfers = self.active_transfers.read().await;
        transfers.keys().cloned().collect()
    }
}

use std::collections::HashMap;