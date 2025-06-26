use crate::quic::stream_manager::StreamManager;
use crate::{FileshareError, Result};
use dashmap::DashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicBool, Ordering};
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt, AsyncSeekExt};
use tokio::sync::Semaphore;
use tracing::{debug, error, info};
use uuid::Uuid;
use quinn::{SendStream, RecvStream};
#[cfg(unix)]
use std::os::unix::fs::FileExt;
#[cfg(windows)]
use std::os::windows::fs::FileExt;

// Optimized constants for maximum performance
const SMALL_FILE_THRESHOLD: u64 = 10 * 1024 * 1024; // 10MB
const OPTIMAL_CHUNK_SIZE: u64 = 16 * 1024 * 1024; // 16MB default for blazing speed
const MAX_PARALLEL_STREAMS: usize = 64; // More streams for better parallelism
const MAX_MEMORY_BUFFER: usize = 256 * 1024 * 1024; // 256MB max memory usage
const WRITE_CONCURRENCY: usize = 32; // Concurrent disk writes

pub struct BlazingTransfer;

impl BlazingTransfer {
    /// Transfer file with maximum performance
    pub async fn transfer_file(
        stream_manager: Arc<StreamManager>,
        source_path: PathBuf,
        target_path: String,
        _peer_id: Uuid,
    ) -> Result<()> {
        let start_time = Instant::now();
        
        // Get file metadata
        let metadata = tokio::fs::metadata(&source_path).await?;
        let file_size = metadata.len();
        let filename = source_path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();
        
        info!("🚀 Starting BLAZING transfer: {} ({:.1} MB)", 
              filename, file_size as f64 / (1024.0 * 1024.0));
        
        // Choose transfer method based on file size
        let result = if file_size <= SMALL_FILE_THRESHOLD {
            Self::single_stream_transfer(stream_manager, source_path, target_path, filename, file_size).await
        } else {
            Self::blazing_parallel_transfer(stream_manager, source_path, target_path, filename, file_size).await
        };
        
        match result {
            Ok(()) => {
                let duration = start_time.elapsed();
                let speed_mbps = (file_size as f64 * 8.0) / (duration.as_secs_f64() * 1_000_000.0);
                info!("✅ BLAZING transfer complete: {:.1} MB in {:.2}s ({:.1} Mbps)", 
                      file_size as f64 / (1024.0 * 1024.0), duration.as_secs_f64(), speed_mbps);
            }
            Err(e) => {
                error!("❌ Transfer failed: {}", e);
                return Err(e);
            }
        }
        
        Ok(())
    }
    
    /// Single stream transfer for small files
    async fn single_stream_transfer(
        stream_manager: Arc<StreamManager>,
        source_path: PathBuf,
        target_path: String,
        filename: String,
        file_size: u64,
    ) -> Result<()> {
        info!("📊 Using single stream for small file ({:.1} MB)", file_size as f64 / (1024.0 * 1024.0));
        
        let mut stream = stream_manager.open_file_transfer_streams(1).await?
            .into_iter().next()
            .ok_or_else(|| FileshareError::Transfer("Failed to create stream".to_string()))?;
        
        // Send header
        let header = format!("BLAZING_SINGLE|{}|{}|{}", filename, file_size, target_path);
        let header_bytes = header.as_bytes();
        stream.write_all(&(header_bytes.len() as u32).to_be_bytes()).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to write header length: {}", e)))?;
        stream.write_all(header_bytes).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to write header: {}", e)))?;
        
        // Transfer file with optimal buffer
        let mut file = tokio::fs::File::open(&source_path).await?;
        let mut buffer = vec![0u8; OPTIMAL_CHUNK_SIZE as usize];
        
        loop {
            match file.read(&mut buffer).await {
                Ok(0) => break,
                Ok(n) => {
                    stream.write_all(&buffer[0..n]).await
                        .map_err(|e| FileshareError::Transfer(format!("Failed to write data: {}", e)))?;
                }
                Err(e) => return Err(FileshareError::FileOperation(format!("Read error: {}", e))),
            }
        }
        
        stream.finish()
            .map_err(|e| FileshareError::Transfer(format!("Failed to finish stream: {}", e)))?;
        Ok(())
    }
    
    /// Blazing fast parallel transfer for large files
    async fn blazing_parallel_transfer(
        stream_manager: Arc<StreamManager>,
        source_path: PathBuf,
        target_path: String,
        filename: String,
        file_size: u64,
    ) -> Result<()> {
        // Calculate optimal parameters
        let chunk_size = Self::calculate_optimal_chunk_size(file_size);
        let total_chunks = (file_size + chunk_size - 1) / chunk_size;
        let stream_count = std::cmp::min(MAX_PARALLEL_STREAMS, total_chunks as usize);
        
        info!("🚀 BLAZING parallel transfer: {} streams, {} chunks of {:.1} MB each", 
              stream_count, total_chunks, chunk_size as f64 / (1024.0 * 1024.0));
        
        // Send control message and get transfer ID
        let control_stream = stream_manager.open_file_transfer_streams(1).await?
            .into_iter().next()
            .ok_or_else(|| FileshareError::Transfer("Failed to create control stream".to_string()))?;
        
        let transfer_id = Uuid::new_v4().to_string();
        Self::send_control_message(control_stream, &filename, file_size, &target_path, chunk_size, total_chunks, &transfer_id).await?;
        
        // Open data streams
        let data_streams = stream_manager.open_file_transfer_streams(stream_count).await?;
        
        // Create shared state for progress tracking
        let bytes_sent = Arc::new(AtomicU64::new(0));
        let start_time = Instant::now();
        
        // Launch parallel senders
        let mut handles = Vec::new();
        let file_path = Arc::new(source_path);
        let transfer_id = Arc::new(transfer_id);
        
        // Better chunk distribution for even load balancing
        let base_chunks = total_chunks / stream_count as u64;
        let extra_chunks = total_chunks % stream_count as u64;
        
        for (stream_idx, stream) in data_streams.into_iter().enumerate() {
            let file_path = file_path.clone();
            let bytes_sent = bytes_sent.clone();
            let transfer_id = transfer_id.clone();
            
            let start_chunk = if (stream_idx as u64) < extra_chunks {
                stream_idx as u64 * (base_chunks + 1)
            } else {
                extra_chunks * (base_chunks + 1) + (stream_idx as u64 - extra_chunks) * base_chunks
            };
            
            let chunks_for_this_stream = if (stream_idx as u64) < extra_chunks {
                base_chunks + 1
            } else {
                base_chunks
            };
            
            let end_chunk = start_chunk + chunks_for_this_stream;
            
            if start_chunk >= total_chunks {
                continue;
            }
            
            let handle = tokio::spawn(async move {
                Self::send_chunks_blazing(
                    stream,
                    file_path,
                    start_chunk,
                    end_chunk,
                    chunk_size,
                    file_size,
                    stream_idx,
                    bytes_sent,
                    &transfer_id,
                ).await
            });
            
            handles.push(handle);
        }
        
        // Monitor progress
        let monitor_handle = tokio::spawn(async move {
            let mut last_bytes = 0u64;
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                let current_bytes = bytes_sent.load(Ordering::Relaxed);
                let elapsed = start_time.elapsed().as_secs_f64();
                let speed_mbps = ((current_bytes - last_bytes) as f64 * 8.0) / 1_000_000.0;
                let progress = (current_bytes as f64 / file_size as f64) * 100.0;
                
                if current_bytes < file_size {
                    info!("📊 Progress: {:.1}% - Speed: {:.1} Mbps", progress, speed_mbps);
                }
                
                last_bytes = current_bytes;
                if current_bytes >= file_size {
                    break;
                }
            }
        });
        
        // Wait for completion
        for handle in handles {
            match handle.await {
                Ok(Ok(())) => {},
                Ok(Err(e)) => return Err(e),
                Err(e) => return Err(FileshareError::Transfer(format!("Task failed: {}", e))),
            }
        }
        
        monitor_handle.abort();
        Ok(())
    }
    
    /// Send control message with transfer ID
    async fn send_control_message(
        mut stream: SendStream,
        filename: &str,
        file_size: u64,
        target_path: &str,
        chunk_size: u64,
        total_chunks: u64,
        transfer_id: &str,
    ) -> Result<()> {
        let header = format!("BLAZING_PARALLEL|{}|{}|{}|{}|{}|{}", 
                           filename, file_size, target_path, chunk_size, total_chunks, transfer_id);
        let header_bytes = header.as_bytes();
        
        stream.write_all(&(header_bytes.len() as u32).to_be_bytes()).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to write control header length: {}", e)))?;
        stream.write_all(header_bytes).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to write control header: {}", e)))?;
        stream.finish()
            .map_err(|e| FileshareError::Transfer(format!("Failed to finish stream: {}", e)))?;
        
        info!("✅ Control message sent for {} chunks (ID: {})", total_chunks, transfer_id);
        Ok(())
    }
    
    /// Send chunks with maximum performance - using special protocol for data streams
    async fn send_chunks_blazing(
        mut stream: SendStream,
        file_path: Arc<PathBuf>,
        start_chunk: u64,
        end_chunk: u64,
        chunk_size: u64,
        file_size: u64,
        stream_idx: usize,
        bytes_sent: Arc<AtomicU64>,
        transfer_id: &str,
    ) -> Result<()> {
        // Send a special marker to indicate this is a data stream with transfer ID
        // Format: MAGIC_BYTES (4) + transfer_id length (4) + transfer_id + chunks
        const MAGIC_BYTES: [u8; 4] = [0xDA, 0x7A, 0x57, 0x12]; // "DATA_STREAM" magic
        stream.write_all(&MAGIC_BYTES).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to write magic bytes: {}", e)))?;
        
        let id_bytes = transfer_id.as_bytes();
        stream.write_all(&(id_bytes.len() as u32).to_be_bytes()).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to write transfer ID length: {}", e)))?;
        stream.write_all(id_bytes).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to write transfer ID: {}", e)))?;
        
        let mut file = tokio::fs::File::open(file_path.as_ref()).await?;
        let mut buffer = vec![0u8; chunk_size as usize];
        
        for chunk_idx in start_chunk..end_chunk {
            let chunk_offset = chunk_idx * chunk_size;
            file.seek(std::io::SeekFrom::Start(chunk_offset)).await?;
            
            let remaining = file_size.saturating_sub(chunk_offset);
            let bytes_to_read = std::cmp::min(chunk_size, remaining) as usize;
            
            file.read_exact(&mut buffer[..bytes_to_read]).await
                .map_err(|e| FileshareError::FileOperation(format!("Failed to read file chunk: {}", e)))?;
            
            // Send: [chunk_id: u64][size: u32][data]
            stream.write_all(&chunk_idx.to_be_bytes()).await
                .map_err(|e| FileshareError::Transfer(format!("Failed to write chunk ID: {}", e)))?;
            stream.write_all(&(bytes_to_read as u32).to_be_bytes()).await
                .map_err(|e| FileshareError::Transfer(format!("Failed to write chunk size: {}", e)))?;
            stream.write_all(&buffer[..bytes_to_read]).await
                .map_err(|e| FileshareError::Transfer(format!("Failed to write chunk data: {}", e)))?;
            
            bytes_sent.fetch_add(bytes_to_read as u64, Ordering::Relaxed);
        }
        
        stream.finish()
            .map_err(|e| FileshareError::Transfer(format!("Failed to finish stream: {}", e)))?;
        debug!("✅ Stream {} completed: chunks {}-{}", stream_idx, start_chunk, end_chunk);
        Ok(())
    }
    
    /// Calculate optimal chunk size for maximum throughput
    fn calculate_optimal_chunk_size(file_size: u64) -> u64 {
        match file_size {
            0..=100_000_000 => 8 * 1024 * 1024,           // <= 100MB: 8MB chunks
            100_000_001..=500_000_000 => 16 * 1024 * 1024,  // 100-500MB: 16MB chunks
            500_000_001..=2_000_000_000 => 32 * 1024 * 1024, // 500MB-2GB: 32MB chunks
            _ => 64 * 1024 * 1024,                        // > 2GB: 64MB chunks
        }
    }
}

/// Blazing fast receiver with true parallel writes
pub struct BlazingReceiver;

// Per-transfer state with lock-free file access on Unix
struct TransferState {
    filename: String,
    file_size: u64,
    chunk_size: u64,
    total_chunks: u64,
    target_path: PathBuf,
    #[cfg(unix)]
    file: Arc<std::fs::File>, // Use std::fs::File for pwrite on Unix
    #[cfg(windows)]
    file: Arc<std::sync::Mutex<std::fs::File>>, // Need mutex on Windows
    received_chunks: AtomicU64,
    write_semaphore: Arc<Semaphore>,
    completed: AtomicBool,
    transfer_id: String,
}

// Use DashMap for lock-free concurrent access
lazy_static::lazy_static! {
    static ref ACTIVE_TRANSFERS: DashMap<String, Arc<TransferState>> = DashMap::new();
}

impl BlazingReceiver {
    pub async fn handle_incoming_transfer(mut recv_stream: RecvStream) -> Result<()> {
        // First, check if this is a data stream by looking for magic bytes
        let mut first_bytes = [0u8; 4];
        recv_stream.read_exact(&mut first_bytes).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to read initial bytes: {}", e)))?;
        
        // Check for data stream magic bytes
        if first_bytes == [0xDA, 0x7A, 0x57, 0x12] {
            // This is a data stream
            return Self::process_data_stream_direct(recv_stream).await;
        }
        
        // Otherwise, it's a control stream with header length
        let header_len = u32::from_be_bytes(first_bytes) as usize;
        
        if header_len > 1000 {
            return Err(FileshareError::Transfer("Invalid header length".to_string()));
        }
        
        // Read header
        let mut header_bytes = vec![0u8; header_len];
        recv_stream.read_exact(&mut header_bytes).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to read header: {}", e)))?;
        let header = String::from_utf8(header_bytes)
            .map_err(|_| FileshareError::Transfer("Invalid UTF-8 in header".to_string()))?;
        
        let parts: Vec<&str> = header.split('|').collect();
        match parts[0] {
            "BLAZING_SINGLE" => Self::receive_single_stream(recv_stream, &parts).await,
            "BLAZING_PARALLEL" => Self::receive_parallel_control(recv_stream, &parts).await,
            _ => Err(FileshareError::Transfer("Invalid header".to_string())),
        }
    }
    
    /// Handle single stream transfer
    async fn receive_single_stream(
        mut recv_stream: RecvStream,
        parts: &[&str],
    ) -> Result<()> {
        if parts.len() < 4 {
            return Err(FileshareError::Transfer("Invalid single transfer header".to_string()));
        }
        
        let filename = parts[1];
        let file_size: u64 = parts[2].parse()
            .map_err(|_| FileshareError::Transfer("Invalid file size".to_string()))?;
        let target_path = PathBuf::from(parts[3]);
        
        info!("📥 Receiving single stream: {} ({:.1} MB)", filename, file_size as f64 / (1024.0 * 1024.0));
        
        // Create file with pre-allocation
        let mut file = tokio::fs::File::create(&target_path).await?;
        file.set_len(file_size).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to pre-allocate file: {}", e)))?;
        
        // Receive data
        let mut buffer = vec![0u8; OPTIMAL_CHUNK_SIZE as usize];
        let mut total_received = 0u64;
        
        while total_received < file_size {
            match recv_stream.read(&mut buffer).await {
                Ok(Some(n)) => {
                    file.write_all(&buffer[..n]).await
                        .map_err(|e| FileshareError::Transfer(format!("Failed to write to file: {}", e)))?;
                    total_received += n as u64;
                }
                Ok(None) => break,
                Err(e) => return Err(FileshareError::Transfer(format!("Read error: {}", e))),
            }
        }
        
        file.sync_all().await
            .map_err(|e| FileshareError::Transfer(format!("Failed to sync file: {}", e)))?;
        info!("✅ Single stream transfer complete: {}", filename);
        Ok(())
    }
    
    /// Initialize parallel transfer with lock-free file access
    async fn receive_parallel_control(
        _recv_stream: RecvStream,
        parts: &[&str],
    ) -> Result<()> {
        if parts.len() < 7 {
            return Err(FileshareError::Transfer("Invalid parallel header".to_string()));
        }
        
        let filename = parts[1].to_string();
        let file_size: u64 = parts[2].parse()
            .map_err(|_| FileshareError::Transfer("Invalid file size".to_string()))?;
        let target_path = PathBuf::from(parts[3]);
        let chunk_size: u64 = parts[4].parse()
            .map_err(|_| FileshareError::Transfer("Invalid chunk size".to_string()))?;
        let total_chunks: u64 = parts[5].parse()
            .map_err(|_| FileshareError::Transfer("Invalid total chunks".to_string()))?;
        let transfer_id = parts[6].to_string();
        
        info!("🎛️ BLAZING parallel receive: {} ({:.1} MB, {} chunks, ID: {})", 
              filename, file_size as f64 / (1024.0 * 1024.0), total_chunks, transfer_id);
        
        // Create file with pre-allocation using std::fs for pwrite support
        let file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .read(true)
            .open(&target_path)
            .map_err(|e| FileshareError::FileOperation(format!("Failed to create file: {}", e)))?;
        
        file.set_len(file_size)
            .map_err(|e| FileshareError::Transfer(format!("Failed to pre-allocate file: {}", e)))?;
        file.sync_all()
            .map_err(|e| FileshareError::Transfer(format!("Failed to sync file: {}", e)))?;
        
        // Create transfer state with platform-specific file handling
        let state = Arc::new(TransferState {
            filename: filename.clone(),
            file_size,
            chunk_size,
            total_chunks,
            target_path,
            #[cfg(unix)]
            file: Arc::new(file),
            #[cfg(windows)]
            file: Arc::new(std::sync::Mutex::new(file)),
            received_chunks: AtomicU64::new(0),
            write_semaphore: Arc::new(Semaphore::new(WRITE_CONCURRENCY)),
            completed: AtomicBool::new(false),
            transfer_id: transfer_id.clone(),
        });
        
        ACTIVE_TRANSFERS.insert(transfer_id, state);
        Ok(())
    }
    
    /// Process data stream directly with magic bytes protocol
    async fn process_data_stream_direct(
        mut recv_stream: RecvStream,
    ) -> Result<()> {
        // Read transfer ID length and ID
        let mut id_len_bytes = [0u8; 4];
        recv_stream.read_exact(&mut id_len_bytes).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to read transfer ID length: {}", e)))?;
        let id_len = u32::from_be_bytes(id_len_bytes) as usize;
        
        let mut id_bytes = vec![0u8; id_len];
        recv_stream.read_exact(&mut id_bytes).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to read transfer ID: {}", e)))?;
        let transfer_id = String::from_utf8(id_bytes)
            .map_err(|_| FileshareError::Transfer("Invalid UTF-8 in transfer ID".to_string()))?;
        
        let state = ACTIVE_TRANSFERS.get(&transfer_id)
            .ok_or_else(|| FileshareError::Transfer(format!("Transfer {} not found", transfer_id)))?
            .clone();
        
        // Process chunks directly without spawning tasks
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
            
            let mut chunk_data = vec![0u8; chunk_size];
            recv_stream.read_exact(&mut chunk_data).await
                .map_err(|e| FileshareError::Transfer(format!("Failed to read chunk data: {}", e)))?;
            
            // Write chunk directly with pwrite (no mutex needed)
            Self::write_chunk_direct(state.clone(), chunk_id, chunk_data).await?;
        }
        
        Ok(())
    }
    
    /// Write chunk directly with pwrite for true parallelism (no mutex)
    async fn write_chunk_direct(
        state: Arc<TransferState>,
        chunk_id: u64,
        data: Vec<u8>,
    ) -> Result<()> {
        // Acquire write permit to control memory usage
        let _permit = state.write_semaphore.acquire().await
            .map_err(|_| FileshareError::Transfer("Failed to acquire write permit".to_string()))?;
        
        let offset = chunk_id * state.chunk_size;
        let file = state.file.clone();
        let state_clone = state.clone();
        
        // Use blocking task for pwrite since it's not async
        tokio::task::spawn_blocking(move || {
            // Use pwrite for position-independent parallel writes
            #[cfg(unix)]
            {
                file.write_at(&data, offset)
                    .map_err(|e| FileshareError::Transfer(format!("Failed to write chunk {}: {}", chunk_id, e)))
            }
            #[cfg(windows)]
            {
                use std::io::{Seek, Write, SeekFrom};
                let mut file = file.lock().unwrap();
                file.seek(SeekFrom::Start(offset))
                    .map_err(|e| FileshareError::Transfer(format!("Failed to seek: {}", e)))?;
                file.write_all(&data)
                    .map_err(|e| FileshareError::Transfer(format!("Failed to write chunk {}: {}", chunk_id, e)))
            }
        })
        .await
        .map_err(|e| FileshareError::Transfer(format!("Task join error: {}", e)))??;
        
        // Update progress
        let received = state.received_chunks.fetch_add(1, Ordering::Relaxed) + 1;
        
        if received % 10 == 0 || received == state.total_chunks {
            let progress = (received as f64 / state.total_chunks as f64) * 100.0;
            info!("📊 Progress: {:.1}% ({}/{})", progress, received, state.total_chunks);
        }
        
        // Check completion
        if received == state.total_chunks {
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
                    let file = file.lock().unwrap();
                    file.sync_all()
                        .map_err(|e| FileshareError::Transfer(format!("Failed to sync file: {}", e)))
                }
            })
            .await
            .map_err(|e| FileshareError::Transfer(format!("Task join error: {}", e)))??;
            
            state.completed.store(true, Ordering::Relaxed);
            info!("🎉 BLAZING transfer complete: {}", filename);
            
            // Cleanup
            ACTIVE_TRANSFERS.remove(&transfer_id);
        }
        
        Ok(())
    }
}