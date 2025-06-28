use crate::quic::stream_manager::StreamManager;
use crate::{FileshareError, Result};
use quinn::{SendStream, RecvStream, TransportConfig, VarInt};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicBool, Ordering};
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt, AsyncSeekExt};
use tracing::{debug, error, info};
use uuid::Uuid;
use futures::future::join_all;
// use bytes::BytesMut; // Not needed
use dashmap::DashMap;

// Ultra-optimized constants
const ULTRA_CHUNK_SIZE: u64 = 128 * 1024 * 1024; // 128MB chunks for maximum throughput
const MAX_PARALLEL_STREAMS: usize = 256; // Much higher stream concurrency
const STREAM_BUFFER_SIZE: usize = 16 * 1024 * 1024; // 16MB per-stream buffer
const WRITE_CONCURRENCY: usize = 8; // Reduced to prevent disk I/O contention

pub struct UltraTransfer;

impl UltraTransfer {
    /// Configure QUIC transport for maximum throughput
    pub fn create_optimized_transport_config() -> TransportConfig {
        let mut config = TransportConfig::default();
        
        // Maximize flow control windows
        config.max_idle_timeout(Some(VarInt::from_u32(300_000).into())); // 5 minutes
        config.stream_receive_window(VarInt::from_u32(256 * 1024 * 1024)); // 256MB per stream
        config.receive_window(VarInt::from_u32(2048 * 1024 * 1024)); // 2GB connection window
        config.send_window(2048 * 1024 * 1024); // 2GB send window
        
        // Optimize for throughput
        config.max_concurrent_bidi_streams(VarInt::from_u32(1024));
        config.max_concurrent_uni_streams(VarInt::from_u32(1024));
        config.datagram_receive_buffer_size(Some(128 * 1024 * 1024)); // 128MB
        
        // Disable features that add latency
        config.keep_alive_interval(None);
        config.congestion_controller_factory(Arc::new(quinn::congestion::BbrConfig::default()));
        
        config
    }
    
    /// Ultra-fast parallel transfer with zero-copy and prefetching
    pub async fn transfer_file(
        stream_manager: Arc<StreamManager>,
        source_path: PathBuf,
        target_path: String,
        _peer_id: Uuid,
    ) -> Result<()> {
        let start_time = Instant::now();
        let metadata = tokio::fs::metadata(&source_path).await?;
        let file_size = metadata.len();
        let filename = source_path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();
        
        info!("⚡ Starting ULTRA transfer: {} ({:.1} MB)", 
              filename, file_size as f64 / (1024.0 * 1024.0));
        
        // Use ultra-optimized chunk size
        let chunk_size = Self::calculate_optimal_chunk_size(file_size);
        let total_chunks = (file_size + chunk_size - 1) / chunk_size;
        let stream_count = std::cmp::min(MAX_PARALLEL_STREAMS, total_chunks as usize).max(1);
        
        info!("⚡ ULTRA config: {} streams, {} chunks of {:.1} MB each", 
              stream_count, total_chunks, chunk_size as f64 / (1024.0 * 1024.0));
        
        // Send metadata first and wait for it to be processed
        let transfer_id = Uuid::new_v4();
        let mut metadata_stream = stream_manager.open_file_transfer_streams(1).await?
            .into_iter().next()
            .ok_or_else(|| FileshareError::Transfer("Failed to create metadata stream".to_string()))?;
        
        Self::send_metadata(&mut metadata_stream, &filename, file_size, &target_path, chunk_size, total_chunks, &transfer_id).await?;
        
        // Wait a bit for metadata to be processed on receiver side
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        
        // Open all data streams in parallel - KEY OPTIMIZATION!
        let stream_futures: Vec<_> = (0..stream_count)
            .map(|_| {
                let sm = stream_manager.clone();
                async move { sm.open_file_transfer_streams(1).await }
            })
            .collect();
        
        let stream_results = join_all(stream_futures).await;
        let mut data_streams = Vec::with_capacity(stream_count);
        for result in stream_results {
            data_streams.extend(result?);
        }
        
        info!("⚡ Opened {} streams in parallel", data_streams.len());
        
        // Create shared state
        let bytes_sent = Arc::new(AtomicU64::new(0));
        let file_path = Arc::new(source_path);
        
        // Launch parallel senders with better load balancing
        let mut handles = Vec::new();
        let chunks_per_stream = total_chunks / stream_count as u64;
        let extra_chunks = total_chunks % stream_count as u64;
        
        for (idx, stream) in data_streams.into_iter().enumerate() {
            let file_path = file_path.clone();
            let bytes_sent = bytes_sent.clone();
            let transfer_id = transfer_id;
            
            // Better chunk distribution
            let start_chunk = if idx < extra_chunks as usize {
                idx as u64 * (chunks_per_stream + 1)
            } else {
                extra_chunks * (chunks_per_stream + 1) + (idx as u64 - extra_chunks) * chunks_per_stream
            };
            
            let chunks_for_this_stream = if idx < extra_chunks as usize {
                chunks_per_stream + 1
            } else {
                chunks_per_stream
            };
            
            let end_chunk = start_chunk + chunks_for_this_stream;
            
            if start_chunk >= total_chunks {
                continue;
            }
            
            let handle = tokio::spawn(async move {
                Self::send_chunks_ultra(
                    stream,
                    file_path,
                    start_chunk,
                    end_chunk.min(total_chunks),
                    chunk_size,
                    file_size,
                    idx,
                    bytes_sent,
                    transfer_id,
                ).await
            });
            
            handles.push(handle);
        }
        
        // Monitor progress
        let bytes_sent_monitor = bytes_sent.clone();
        let monitor_handle = tokio::spawn(async move {
            let mut last_bytes = 0u64;
            let mut last_time = Instant::now();
            
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                let current_bytes = bytes_sent_monitor.load(Ordering::Relaxed);
                let current_time = Instant::now();
                let elapsed = current_time.duration_since(last_time).as_secs_f64();
                
                if elapsed > 0.0 {
                    let speed_mbps = ((current_bytes - last_bytes) as f64 * 8.0) / (elapsed * 1_000_000.0);
                    let progress = (current_bytes as f64 / file_size as f64) * 100.0;
                    
                    if current_bytes < file_size {
                        info!("⚡ Progress: {:.1}% - Speed: {:.1} Mbps", progress, speed_mbps);
                    }
                }
                
                last_bytes = current_bytes;
                last_time = current_time;
                
                if current_bytes >= file_size {
                    break;
                }
            }
        });
        
        // Wait for all transfers with better error handling
        let mut transfer_failed = false;
        for (idx, handle) in handles.into_iter().enumerate() {
            match handle.await {
                Ok(Ok(())) => {
                    debug!("Stream {} completed successfully", idx);
                },
                Ok(Err(e)) => {
                    error!("Stream {} failed: {}", idx, e);
                    transfer_failed = true;
                },
                Err(e) => {
                    error!("Stream {} task panicked: {}", idx, e);
                    transfer_failed = true;
                }
            }
        }
        
        if transfer_failed {
            return Err(FileshareError::Transfer("One or more streams failed".to_string()));
        }
        
        monitor_handle.abort();
        
        let duration = start_time.elapsed();
        let throughput_mbps = (file_size as f64 * 8.0) / (duration.as_secs_f64() * 1_000_000.0);
        info!("⚡ ULTRA transfer complete: {:.1} MB in {:.2}s ({:.1} Mbps)", 
              file_size as f64 / (1024.0 * 1024.0), duration.as_secs_f64(), throughput_mbps);
        
        Ok(())
    }
    
    async fn send_metadata(
        stream: &mut SendStream,
        filename: &str,
        file_size: u64,
        target_path: &str,
        chunk_size: u64,
        total_chunks: u64,
        transfer_id: &Uuid,
    ) -> Result<()> {
        // Simple, efficient protocol
        let metadata = format!("ULTRA|{}|{}|{}|{}|{}|{}", 
                             filename, file_size, target_path, chunk_size, total_chunks, transfer_id);
        let metadata_bytes = metadata.as_bytes();
        
        stream.write_all(&(metadata_bytes.len() as u32).to_be_bytes()).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to write metadata length: {}", e)))?;
        stream.write_all(metadata_bytes).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to write metadata: {}", e)))?;
        stream.finish()
            .map_err(|e| FileshareError::Transfer(format!("Failed to finish metadata stream: {}", e)))?;
        
        info!("✅ Sent ULTRA metadata for transfer {}", transfer_id);
        Ok(())
    }
    
    async fn send_chunks_ultra(
        mut stream: SendStream,
        file_path: Arc<PathBuf>,
        start_chunk: u64,
        end_chunk: u64,
        chunk_size: u64,
        file_size: u64,
        stream_idx: usize,
        bytes_sent: Arc<AtomicU64>,
        transfer_id: Uuid,
    ) -> Result<()> {
        // Send transfer ID first
        stream.write_all(transfer_id.as_bytes()).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to write transfer ID: {}", e)))?;
        
        // Use buffered reader for better performance
        let file = tokio::fs::File::open(file_path.as_ref()).await?;
        let mut file = tokio::io::BufReader::with_capacity(STREAM_BUFFER_SIZE, file);
        
        // Pre-allocate buffer
        let mut buffer = vec![0u8; chunk_size as usize];
        
        for chunk_idx in start_chunk..end_chunk {
            let offset = chunk_idx * chunk_size;
            let bytes_to_read = std::cmp::min(chunk_size, file_size.saturating_sub(offset)) as usize;
            
            if bytes_to_read == 0 {
                break;
            }
            
            // Seek and read
            file.seek(std::io::SeekFrom::Start(offset)).await
                .map_err(|e| FileshareError::FileOperation(format!("Seek failed: {}", e)))?;
            file.read_exact(&mut buffer[..bytes_to_read]).await
                .map_err(|e| FileshareError::FileOperation(format!("Read failed: {}", e)))?;
            
            // Send chunk header and data with detailed error info
            stream.write_all(&chunk_idx.to_be_bytes()).await
                .map_err(|e| FileshareError::Transfer(format!("Failed to write chunk ID {} on stream {}: {}", chunk_idx, stream_idx, e)))?;
            stream.write_all(&(bytes_to_read as u32).to_be_bytes()).await
                .map_err(|e| FileshareError::Transfer(format!("Failed to write chunk size {} on stream {}: {}", bytes_to_read, stream_idx, e)))?;
            stream.write_all(&buffer[..bytes_to_read]).await
                .map_err(|e| FileshareError::Transfer(format!("Failed to write chunk data {} bytes on stream {}: {}", bytes_to_read, stream_idx, e)))?;
            
            bytes_sent.fetch_add(bytes_to_read as u64, Ordering::Relaxed);
            
            if chunk_idx % 5 == 0 {
                debug!("Stream {} sent chunk {}/{}", stream_idx, chunk_idx - start_chunk + 1, end_chunk - start_chunk);
            }
        }
        
        stream.finish()
            .map_err(|e| FileshareError::Transfer(format!("Failed to finish stream: {}", e)))?;
        
        debug!("✅ Stream {} completed: chunks {}-{}", stream_idx, start_chunk, end_chunk);
        Ok(())
    }
    
    /// Calculate optimal chunk size for maximum throughput (smaller chunks for better disk I/O)
    fn calculate_optimal_chunk_size(file_size: u64) -> u64 {
        match file_size {
            0..=100_000_000 => 16 * 1024 * 1024,           // <= 100MB: 16MB chunks
            100_000_001..=500_000_000 => 32 * 1024 * 1024,  // 100-500MB: 32MB chunks
            500_000_001..=2_000_000_000 => 64 * 1024 * 1024, // 500MB-2GB: 64MB chunks
            _ => 128 * 1024 * 1024,                        // > 2GB: 128MB chunks
        }
    }
}

/// Ultra-fast receiver with zero-copy writes
pub struct UltraReceiver;

// Per-transfer state with lock-free file access
struct UltraTransferState {
    filename: String,
    file_size: u64,
    chunk_size: u64,
    total_chunks: u64,
    target_path: PathBuf,
    #[cfg(unix)]
    file: Arc<std::fs::File>, // Use std::fs::File for pwrite on Unix
    #[cfg(windows)]
    file: Arc<tokio::sync::Mutex<std::fs::File>>, // Need mutex on Windows
    received_chunks: AtomicU64,
    write_semaphore: Arc<tokio::sync::Semaphore>,
    completed: AtomicBool,
    transfer_id: String,
}

// Global transfer registry
lazy_static::lazy_static! {
    static ref ULTRA_TRANSFERS: DashMap<String, Arc<UltraTransferState>> = DashMap::new();
}

impl UltraReceiver {
    pub async fn handle_incoming_transfer(mut recv_stream: RecvStream) -> Result<()> {
        // Try to read first 4 bytes to determine stream type
        let mut first_bytes = [0u8; 4];
        recv_stream.read_exact(&mut first_bytes).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to read initial bytes: {}", e)))?;
        
        // Check if this is a metadata stream (starts with length) or data stream (starts with UUID)
        let header_len = u32::from_be_bytes(first_bytes) as usize;
        
        if header_len > 0 && header_len < 10000 { // Reasonable header length
            // This is a metadata stream
            let mut header_bytes = vec![0u8; header_len];
            recv_stream.read_exact(&mut header_bytes).await
                .map_err(|e| FileshareError::Transfer(format!("Failed to read header: {}", e)))?;
            
            let header = String::from_utf8(header_bytes)
                .map_err(|_| FileshareError::Transfer("Invalid UTF-8 in header".to_string()))?;
            
            let parts: Vec<&str> = header.split('|').collect();
            if parts.len() >= 7 && parts[0] == "ULTRA" {
                return Self::receive_ultra_metadata(recv_stream, &parts).await;
            } else {
                // Not an ultra transfer, try to fall back to blazing
                debug!("Not an ULTRA transfer, header starts with: {}", parts.get(0).map_or("", |v| *v));
                return Err(FileshareError::Transfer("Not an ULTRA transfer".to_string()));
            }
        } else {
            // This might be a data stream - try to read rest of UUID
            let mut rest_of_uuid = [0u8; 12];
            recv_stream.read_exact(&mut rest_of_uuid).await
                .map_err(|e| FileshareError::Transfer(format!("Failed to read rest of UUID: {}", e)))?;
            
            let mut uuid_bytes = [0u8; 16];
            uuid_bytes[..4].copy_from_slice(&first_bytes);
            uuid_bytes[4..].copy_from_slice(&rest_of_uuid);
            
            let transfer_id = Uuid::from_bytes(uuid_bytes).to_string();
            return Self::receive_data_stream(recv_stream, transfer_id).await;
        }
    }
    
    async fn receive_ultra_metadata(
        _recv_stream: RecvStream,
        parts: &[&str],
    ) -> Result<()> {
        let filename = parts[1].to_string();
        let file_size: u64 = parts[2].parse()
            .map_err(|_| FileshareError::Transfer("Invalid file size".to_string()))?;
        let target_path = PathBuf::from(parts[3]);
        let chunk_size: u64 = parts[4].parse()
            .map_err(|_| FileshareError::Transfer("Invalid chunk size".to_string()))?;
        let total_chunks: u64 = parts[5].parse()
            .map_err(|_| FileshareError::Transfer("Invalid total chunks".to_string()))?;
        let transfer_id = parts[6].to_string();
        
        info!("⚡ Receiving ULTRA transfer: {} ({:.1} MB, {} chunks)", 
              filename, file_size as f64 / (1024.0 * 1024.0), total_chunks);
        
        // Create file with pre-allocation
        let file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .read(true)
            .truncate(true)
            .open(&target_path)
            .map_err(|e| FileshareError::FileOperation(format!("Failed to create file: {}", e)))?;
        
        file.set_len(file_size)
            .map_err(|e| FileshareError::Transfer(format!("Failed to pre-allocate file: {}", e)))?;
        file.sync_all()
            .map_err(|e| FileshareError::Transfer(format!("Failed to sync file: {}", e)))?;
        
        // Create transfer state
        let state = Arc::new(UltraTransferState {
            filename: filename.clone(),
            file_size,
            chunk_size,
            total_chunks,
            target_path,
            #[cfg(unix)]
            file: Arc::new(file),
            #[cfg(windows)]
            file: Arc::new(tokio::sync::Mutex::new(file)),
            received_chunks: AtomicU64::new(0),
            write_semaphore: Arc::new(tokio::sync::Semaphore::new(WRITE_CONCURRENCY)),
            completed: AtomicBool::new(false),
            transfer_id: transfer_id.clone(),
        });
        
        ULTRA_TRANSFERS.insert(transfer_id.clone(), state);
        info!("✅ Registered ULTRA transfer {}", transfer_id);
        Ok(())
    }
    
    async fn receive_data_stream(
        mut recv_stream: RecvStream,
        transfer_id: String,
    ) -> Result<()> {
        // Wait for transfer to be registered (with timeout)
        let state = {
            let mut attempts = 0;
            loop {
                if let Some(state) = ULTRA_TRANSFERS.get(&transfer_id) {
                    break state.clone();
                }
                
                attempts += 1;
                if attempts > 20 { // 2 seconds max wait
                    error!("Transfer {} not found after {} attempts", transfer_id, attempts);
                    return Err(FileshareError::Transfer(format!("Transfer {} not found after timeout", transfer_id)));
                }
                
                debug!("Waiting for transfer {} to be registered (attempt {})", transfer_id, attempts);
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            }
        };
        
        // Process chunks with smaller buffer for better memory management
        let buffer_size = std::cmp::min(state.chunk_size as usize, 32 * 1024 * 1024); // Max 32MB buffer
        let mut buffer = vec![0u8; buffer_size];
        let mut chunks_received = 0u64;
        
        loop {
            // Read chunk header
            let mut chunk_header = [0u8; 12]; // chunk_id (8) + size (4)
            match recv_stream.read_exact(&mut chunk_header).await {
                Ok(_) => {},
                Err(_) => break, // Stream finished
            }
            
            let chunk_id = u64::from_be_bytes([
                chunk_header[0], chunk_header[1], chunk_header[2], chunk_header[3],
                chunk_header[4], chunk_header[5], chunk_header[6], chunk_header[7],
            ]);
            let chunk_size = u32::from_be_bytes([
                chunk_header[8], chunk_header[9], chunk_header[10], chunk_header[11],
            ]) as usize;
            
            // Ensure we have enough buffer space
            if chunk_size > buffer.len() {
                buffer.resize(chunk_size, 0);
            }
            
            // Read chunk data
            recv_stream.read_exact(&mut buffer[..chunk_size]).await
                .map_err(|e| FileshareError::Transfer(format!("Failed to read chunk data: {}", e)))?;
            
            // Write chunk with pwrite (lock-free on Unix)
            Self::write_chunk_ultra(state.clone(), chunk_id, &buffer[..chunk_size]).await?;
            
            chunks_received += 1;
            if chunks_received % 10 == 0 {
                debug!("Received {} chunks for transfer {}", chunks_received, transfer_id);
            }
        }
        
        Ok(())
    }
    
    async fn write_chunk_ultra(
        state: Arc<UltraTransferState>,
        chunk_id: u64,
        data: &[u8],
    ) -> Result<()> {
        // Acquire write permit to control disk I/O concurrency
        let _permit = state.write_semaphore.acquire().await
            .map_err(|_| FileshareError::Transfer("Failed to acquire write permit".to_string()))?;
        
        let offset = chunk_id * state.chunk_size;
        let data_len = data.len();
        
        // Use blocking task for I/O to avoid blocking async executor
        let file = state.file.clone();
        let data_owned = data.to_vec(); // Copy data for move into blocking task
        let _write_result = tokio::task::spawn_blocking(move || {
            // Platform-specific write with error handling
            #[cfg(unix)]
            {
                use std::os::unix::fs::FileExt;
                file.write_at(&data_owned, offset)
                    .map_err(|e| FileshareError::Transfer(format!("Failed to write chunk {} (offset {}, {} bytes): {}", chunk_id, offset, data_len, e)))
            }
            
            #[cfg(windows)]
            {
                use std::os::windows::fs::FileExt;
                file.seek_write(&data_owned, offset)
                    .map_err(|e| FileshareError::Transfer(format!("Failed to write chunk {} (offset {}, {} bytes): {}", chunk_id, offset, data_len, e)))
            }
        }).await
        .map_err(|e| FileshareError::Transfer(format!("Write task failed: {}", e)))??;
        
        debug!("Written chunk {} ({} bytes) at offset {}", chunk_id, data_len, offset);
        
        // Update progress
        let received = state.received_chunks.fetch_add(1, Ordering::Relaxed) + 1;
        
        if received % 5 == 0 || received == state.total_chunks {
            let progress = (received as f64 / state.total_chunks as f64) * 100.0;
            info!("⚡ Progress: {:.1}% ({}/{})", progress, received, state.total_chunks);
        }
        
        // Check completion
        if received == state.total_chunks && !state.completed.swap(true, Ordering::Relaxed) {
            // Sync file in blocking task
            let file = state.file.clone();
            let filename = state.filename.clone();
            let transfer_id = state.transfer_id.clone();
            
            tokio::task::spawn_blocking(move || {
                #[cfg(unix)]
                {
                    file.sync_all()
                        .map_err(|e| FileshareError::Transfer(format!("Failed to sync file: {}", e)))
                }
                
                #[cfg(windows)]
                {
                    file.sync_all()
                        .map_err(|e| FileshareError::Transfer(format!("Failed to sync file: {}", e)))
                }
            }).await
            .map_err(|e| FileshareError::Transfer(format!("Sync task failed: {}", e)))??;
            
            info!("🎉 ULTRA transfer complete: {}", filename);
            
            // Cleanup
            ULTRA_TRANSFERS.remove(&transfer_id);
        }
        
        Ok(())
    }
}