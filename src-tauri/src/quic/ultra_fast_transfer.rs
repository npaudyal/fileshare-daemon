use crate::quic::stream_manager::StreamManager;
use crate::{FileshareError, Result};
use quinn::{RecvStream, SendStream};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::RwLock;
use tracing::{error, info};
use uuid::Uuid;

// Performance constants
const CHUNK_SIZE: u64 = 32 * 1024 * 1024; // 32MB chunks for optimal throughput
const MAX_STREAMS: usize = 32; // Maximum parallel streams
const BUFFER_SIZE: usize = 64 * 1024; // 64KB buffer for network I/O
const WRITE_BUFFER_POOL_SIZE: usize = 64; // Pre-allocated write buffers

// Protocol constants
const PROTOCOL_VERSION: u8 = 2;
const MSG_CONTROL: u8 = 0x01;
const MSG_DATA: u8 = 0x02;

#[derive(Clone)]
struct TransferInfo {
    filename: String,
    file_size: u64,
    target_path: PathBuf,
    chunk_count: u64,
    start_time: Instant,
    chunks_received: Arc<AtomicU64>,
    completed: Arc<AtomicBool>,
}

pub struct UltraFastTransfer {
    // Buffer pool to reduce allocations
    buffer_pool: Arc<RwLock<Vec<Vec<u8>>>>,
    // Active transfers
    transfers: Arc<RwLock<HashMap<Uuid, TransferInfo>>>,
}

impl UltraFastTransfer {
    pub fn new() -> Self {
        // Pre-allocate buffers
        let mut buffers = Vec::with_capacity(WRITE_BUFFER_POOL_SIZE);
        for _ in 0..WRITE_BUFFER_POOL_SIZE {
            buffers.push(vec![0u8; CHUNK_SIZE as usize]);
        }
        
        Self {
            buffer_pool: Arc::new(RwLock::new(buffers)),
            transfers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn send_file(
        &self,
        stream_manager: Arc<StreamManager>,
        source_path: PathBuf,
        target_path: String,
    ) -> Result<()> {
        let start = Instant::now();
        let metadata = tokio::fs::metadata(&source_path).await?;
        let file_size = metadata.len();
        let filename = source_path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        info!("🚀 Starting ultra-fast transfer: {} ({:.1} MB)", 
            filename, file_size as f64 / (1024.0 * 1024.0));

        let transfer_id = Uuid::new_v4();
        let chunk_count = (file_size + CHUNK_SIZE - 1) / CHUNK_SIZE;
        let stream_count = (chunk_count as usize).min(MAX_STREAMS).max(1);

        // Send control message on first stream
        let mut control_stream = stream_manager
            .open_file_transfer_streams(1)
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| FileshareError::Transfer("Failed to create control stream".to_string()))?;

        self.send_control_message(
            &mut control_stream,
            &transfer_id,
            &filename,
            file_size,
            &target_path,
            chunk_count,
        ).await?;

        // Open all data streams at once
        let data_streams = stream_manager
            .open_file_transfer_streams(stream_count)
            .await?;

        // Use memory-mapped file for zero-copy reads
        let file = Arc::new(tokio::fs::File::open(&source_path).await?);
        let bytes_sent = Arc::new(AtomicU64::new(0));

        // Launch parallel senders with better work distribution
        let mut handles = Vec::new();
        for (idx, mut stream) in data_streams.into_iter().enumerate() {
            let file = file.clone();
            let bytes_sent = bytes_sent.clone();
            let transfer_id = transfer_id;
            
            // Calculate chunks for this stream using round-robin distribution
            let mut chunks = Vec::new();
            let mut chunk_idx = idx as u64;
            while chunk_idx < chunk_count {
                chunks.push(chunk_idx);
                chunk_idx += stream_count as u64;
            }

            // Skip streams with no chunks
            if chunks.is_empty() {
                // Close the empty stream
                if let Err(e) = stream.finish() {
                    error!("Failed to close empty stream: {}", e);
                }
                continue;
            }

            handles.push(tokio::spawn(async move {
                // Pre-allocate buffer for this stream
                let mut buffer = vec![0u8; CHUNK_SIZE as usize];
                
                for chunk_id in chunks {
                    let offset = chunk_id * CHUNK_SIZE;
                    let remaining = file_size.saturating_sub(offset);
                    let chunk_size = remaining.min(CHUNK_SIZE) as usize;

                    // Read chunk
                    use tokio::io::AsyncSeekExt;
                    let mut file_clone = file.try_clone().await?;
                    file_clone.seek(std::io::SeekFrom::Start(offset)).await?;
                    file_clone.read_exact(&mut buffer[..chunk_size]).await?;

                    // Send chunk with minimal protocol overhead
                    stream.write_all(&[MSG_DATA]).await
                        .map_err(|e| FileshareError::Transfer(format!("Failed to write message type: {}", e)))?;
                    stream.write_all(transfer_id.as_bytes()).await
                        .map_err(|e| FileshareError::Transfer(format!("Failed to write transfer ID: {}", e)))?;
                    stream.write_all(&chunk_id.to_le_bytes()).await
                        .map_err(|e| FileshareError::Transfer(format!("Failed to write chunk ID: {}", e)))?;
                    stream.write_all(&(chunk_size as u32).to_le_bytes()).await
                        .map_err(|e| FileshareError::Transfer(format!("Failed to write chunk size: {}", e)))?;
                    stream.write_all(&buffer[..chunk_size]).await
                        .map_err(|e| FileshareError::Transfer(format!("Failed to write chunk data: {}", e)))?;

                    bytes_sent.fetch_add(chunk_size as u64, Ordering::Relaxed);
                }

                stream.finish()
                    .map_err(|e| FileshareError::Transfer(format!("Failed to finish stream: {}", e)))?;
                Ok::<(), FileshareError>(())
            }));
        }

        // Monitor progress
        let monitor = tokio::spawn(async move {
            let mut last_bytes = 0u64;
            loop {
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                let current = bytes_sent.load(Ordering::Relaxed);
                let speed_mbps = ((current - last_bytes) as f64 * 8.0) / 500_000.0;
                let progress = (current as f64 / file_size as f64) * 100.0;
                
                if current < file_size {
                    info!("📊 Progress: {:.1}% - Speed: {:.1} Mbps", progress, speed_mbps);
                }
                
                last_bytes = current;
                if current >= file_size {
                    break;
                }
            }
        });

        // Wait for all streams
        for handle in handles {
            handle.await
                .map_err(|e| FileshareError::Transfer(format!("Task join error: {}", e)))??;
        }
        monitor.abort();

        let duration = start.elapsed();
        let speed_mbps = (file_size as f64 * 8.0) / (duration.as_secs_f64() * 1_000_000.0);
        info!("✅ Transfer complete: {:.1} MB in {:.2}s ({:.1} Mbps)",
            file_size as f64 / (1024.0 * 1024.0),
            duration.as_secs_f64(),
            speed_mbps
        );

        Ok(())
    }

    async fn send_control_message(
        &self,
        stream: &mut SendStream,
        transfer_id: &Uuid,
        filename: &str,
        file_size: u64,
        target_path: &str,
        chunk_count: u64,
    ) -> Result<()> {
        // Simple, efficient protocol
        stream.write_all(&[MSG_CONTROL]).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to write control message type: {}", e)))?;
        stream.write_all(&[PROTOCOL_VERSION]).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to write protocol version: {}", e)))?;
        stream.write_all(transfer_id.as_bytes()).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to write transfer ID: {}", e)))?;
        stream.write_all(&(filename.len() as u16).to_le_bytes()).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to write filename length: {}", e)))?;
        stream.write_all(filename.as_bytes()).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to write filename: {}", e)))?;
        stream.write_all(&file_size.to_le_bytes()).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to write file size: {}", e)))?;
        stream.write_all(&(target_path.len() as u16).to_le_bytes()).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to write target path length: {}", e)))?;
        stream.write_all(target_path.as_bytes()).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to write target path: {}", e)))?;
        stream.write_all(&chunk_count.to_le_bytes()).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to write chunk count: {}", e)))?;
        stream.finish()
            .map_err(|e| FileshareError::Transfer(format!("Failed to finish control stream: {}", e)))?;
        Ok(())
    }

    pub async fn receive_stream(&self, mut stream: RecvStream) -> Result<()> {
        // Read message type
        let mut msg_type = [0u8; 1];
        match stream.read_exact(&mut msg_type).await {
            Ok(_) => {},
            Err(e) => {
                // This might be an empty stream - just return without error
                info!("Stream has no data, likely an empty stream: {}", e);
                return Ok(());
            }
        }

        match msg_type[0] {
            MSG_CONTROL => self.handle_control_stream(stream).await,
            MSG_DATA => self.handle_data_stream(stream).await,
            _ => Err(FileshareError::Transfer("Invalid message type".to_string())),
        }
    }

    async fn handle_control_stream(&self, mut stream: RecvStream) -> Result<()> {
        let mut version = [0u8; 1];
        stream.read_exact(&mut version).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to read protocol version: {}", e)))?;
        
        if version[0] != PROTOCOL_VERSION {
            return Err(FileshareError::Transfer("Protocol version mismatch".to_string()));
        }

        // Read transfer info
        let mut transfer_id = [0u8; 16];
        stream.read_exact(&mut transfer_id).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to read transfer ID: {}", e)))?;
        let transfer_id = Uuid::from_bytes(transfer_id);

        let mut filename_len = [0u8; 2];
        stream.read_exact(&mut filename_len).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to read filename length: {}", e)))?;
        let filename_len = u16::from_le_bytes(filename_len) as usize;
        
        let mut filename = vec![0u8; filename_len];
        stream.read_exact(&mut filename).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to read filename: {}", e)))?;
        let filename = String::from_utf8(filename)
            .map_err(|e| FileshareError::Transfer(format!("Invalid UTF-8 in filename: {}", e)))?;

        let mut file_size = [0u8; 8];
        stream.read_exact(&mut file_size).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to read file size: {}", e)))?;
        let file_size = u64::from_le_bytes(file_size);

        let mut target_len = [0u8; 2];
        stream.read_exact(&mut target_len).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to read target path length: {}", e)))?;
        let target_len = u16::from_le_bytes(target_len) as usize;
        
        let mut target_path = vec![0u8; target_len];
        stream.read_exact(&mut target_path).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to read target path: {}", e)))?;
        let target_path = PathBuf::from(String::from_utf8(target_path)
            .map_err(|e| FileshareError::Transfer(format!("Invalid UTF-8 in target path: {}", e)))?);

        let mut chunk_count = [0u8; 8];
        stream.read_exact(&mut chunk_count).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to read chunk count: {}", e)))?;
        let chunk_count = u64::from_le_bytes(chunk_count);

        info!("📥 Receiving: {} ({:.1} MB, {} chunks)", 
            filename, file_size as f64 / (1024.0 * 1024.0), chunk_count);

        // Create file with pre-allocation
        let file = tokio::fs::File::create(&target_path).await?;
        file.set_len(file_size).await?;
        file.sync_all().await?;
        drop(file);

        // Create transfer info
        let info = TransferInfo {
            filename,
            file_size,
            target_path,
            chunk_count,
            start_time: Instant::now(),
            chunks_received: Arc::new(AtomicU64::new(0)),
            completed: Arc::new(AtomicBool::new(false)),
        };

        self.transfers.write().await.insert(transfer_id, info);
        Ok(())
    }

    async fn handle_data_stream(&self, mut stream: RecvStream) -> Result<()> {
        // Get a buffer from the pool
        let mut buffer = self.get_buffer().await;

        loop {
            // Read transfer ID
            let mut transfer_id = [0u8; 16];
            match stream.read_exact(&mut transfer_id).await {
                Ok(_) => {},
                Err(_) => break, // Stream finished
            }
            let transfer_id = Uuid::from_bytes(transfer_id);

            // Get transfer info
            let transfers = self.transfers.read().await;
            let info = transfers.get(&transfer_id)
                .ok_or_else(|| FileshareError::Transfer("Transfer not found".to_string()))?
                .clone();
            drop(transfers);

            // Read chunk header
            let mut chunk_id = [0u8; 8];
            stream.read_exact(&mut chunk_id).await
                .map_err(|e| FileshareError::Transfer(format!("Failed to read chunk ID: {}", e)))?;
            let chunk_id = u64::from_le_bytes(chunk_id);

            let mut chunk_size = [0u8; 4];
            stream.read_exact(&mut chunk_size).await
                .map_err(|e| FileshareError::Transfer(format!("Failed to read chunk size: {}", e)))?;
            let chunk_size = u32::from_le_bytes(chunk_size) as usize;

            // Read chunk data
            stream.read_exact(&mut buffer[..chunk_size]).await
                .map_err(|e| FileshareError::Transfer(format!("Failed to read chunk data: {}", e)))?;

            // Write chunk directly to disk
            let offset = chunk_id * CHUNK_SIZE;
            self.write_chunk(&info.target_path, offset, &buffer[..chunk_size]).await?;

            // Update progress
            let received = info.chunks_received.fetch_add(1, Ordering::Relaxed) + 1;
            if received == info.chunk_count {
                info.completed.store(true, Ordering::Relaxed);
                let duration = info.start_time.elapsed();
                let speed_mbps = (info.file_size as f64 * 8.0) / (duration.as_secs_f64() * 1_000_000.0);
                info!("✅ Received: {} in {:.2}s ({:.1} Mbps)", 
                    info.filename, duration.as_secs_f64(), speed_mbps);
                
                // Clean up
                self.transfers.write().await.remove(&transfer_id);
            }
        }

        // Return buffer to pool
        self.return_buffer(buffer).await;
        Ok(())
    }

    async fn write_chunk(&self, path: &PathBuf, offset: u64, data: &[u8]) -> Result<()> {
        // Open file in append mode for better performance
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .await?;
        
        file.seek(std::io::SeekFrom::Start(offset)).await?;
        file.write_all(data).await?;
        Ok(())
    }

    async fn get_buffer(&self) -> Vec<u8> {
        let mut pool = self.buffer_pool.write().await;
        pool.pop().unwrap_or_else(|| vec![0u8; CHUNK_SIZE as usize])
    }

    async fn return_buffer(&self, buffer: Vec<u8>) {
        let mut pool = self.buffer_pool.write().await;
        if pool.len() < WRITE_BUFFER_POOL_SIZE {
            pool.push(buffer);
        }
    }
}