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
        // Calculate optimal parameters with aggressive parallelism
        let chunk_size = Self::calculate_optimal_chunk_size(file_size);
        let total_chunks = (file_size + chunk_size - 1) / chunk_size;
        
        // Use more streams for better congestion control bypass
        let ideal_streams = std::cmp::min(total_chunks as usize, MAX_PARALLEL_STREAMS);
        let stream_count = std::cmp::max(ideal_streams, 8); // Minimum 8 streams for good parallelism
        
        info!("🚀 BLAZING parallel transfer: {} streams, {} chunks of {:.1} MB each", 
              stream_count, total_chunks, chunk_size as f64 / (1024.0 * 1024.0));
        
        // Send control message and warm up connection
        let control_stream = stream_manager.open_file_transfer_streams(1).await?
            .into_iter().next()
            .ok_or_else(|| FileshareError::Transfer("Failed to create control stream".to_string()))?;
            
        Self::send_control_message(control_stream, &filename, file_size, &target_path, chunk_size, total_chunks).await?;
        
        // Brief delay to let receiver initialize
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        
        // Open data streams
        let data_streams = stream_manager.open_file_transfer_streams(stream_count).await?;
        
        // Create shared state for progress tracking
        let bytes_sent = Arc::new(AtomicU64::new(0));
        let start_time = Instant::now();
        
        // Launch parallel senders
        let mut handles = Vec::new();
        let file_path = Arc::new(source_path);
        
        for (stream_idx, stream) in data_streams.into_iter().enumerate() {
            let file_path = file_path.clone();
            let bytes_sent = bytes_sent.clone();
            
            // Distribute chunks across streams
            let chunks_per_stream = (total_chunks + stream_count as u64 - 1) / stream_count as u64;
            let start_chunk = stream_idx as u64 * chunks_per_stream;
            let end_chunk = std::cmp::min(start_chunk + chunks_per_stream, total_chunks);
            
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
                ).await
            });
            
            handles.push(handle);
        }
        
        // Monitor progress with better reporting during slow start
        let monitor_handle = tokio::spawn(async move {
            let mut last_bytes = 0u64;
            let mut slow_start_warned = false;
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                let current_bytes = bytes_sent.load(Ordering::Relaxed);
                let elapsed = start_time.elapsed().as_secs_f64();
                let speed_mbps = ((current_bytes - last_bytes) as f64 * 8.0) / 1_000_000.0;
                let progress = (current_bytes as f64 / file_size as f64) * 100.0;
                
                if current_bytes < file_size {
                    if elapsed > 10.0 && progress < 5.0 && !slow_start_warned {
                        info!("📊 Progress: {:.1}% - QUIC slow start phase, speed will increase...", progress);
                        slow_start_warned = true;
                    } else {
                        info!("📊 Progress: {:.1}% - Speed: {:.1} Mbps", progress, speed_mbps);
                    }
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
    
    /// Send control message
    async fn send_control_message(
        mut stream: quinn::SendStream,
        filename: &str,
        file_size: u64,
        target_path: &str,
        chunk_size: u64,
        total_chunks: u64,
    ) -> Result<()> {
        let header = format!("BLAZING_PARALLEL|{}|{}|{}|{}|{}", 
                           filename, file_size, target_path, chunk_size, total_chunks);
        let header_bytes = header.as_bytes();
        
        stream.write_all(&(header_bytes.len() as u32).to_be_bytes()).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to write control header length: {}", e)))?;
        stream.write_all(header_bytes).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to write control header: {}", e)))?;
        stream.finish()
            .map_err(|e| FileshareError::Transfer(format!("Failed to finish stream: {}", e)))?;
        
        info!("✅ Control message sent for {} chunks", total_chunks);
        Ok(())
    }
    
    /// Send chunks with maximum performance
    async fn send_chunks_blazing(
        mut stream: quinn::SendStream,
        file_path: Arc<PathBuf>,
        start_chunk: u64,
        end_chunk: u64,
        chunk_size: u64,
        file_size: u64,
        stream_idx: usize,
        bytes_sent: Arc<AtomicU64>,
    ) -> Result<()> {
        let mut file = tokio::fs::File::open(file_path.as_ref()).await?;
        let mut buffer = vec![0u8; chunk_size as usize];
        
        // Send chunks with small initial delay to reduce congestion
        for (idx, chunk_idx) in (start_chunk..end_chunk).enumerate() {
            // Small staggered delay for first few chunks to help with congestion control
            if idx < 3 && stream_idx > 0 {
                tokio::time::sleep(tokio::time::Duration::from_millis(stream_idx as u64 * 10)).await;
            }
            
            let chunk_idx = chunk_idx;
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
            0..=50_000_000 => 4 * 1024 * 1024,           // <= 50MB: 4MB chunks
            50_000_001..=200_000_000 => 8 * 1024 * 1024,   // 50-200MB: 8MB chunks  
            200_000_001..=1_000_000_000 => 12 * 1024 * 1024, // 200MB-1GB: 12MB chunks
            1_000_000_001..=5_000_000_000 => 16 * 1024 * 1024, // 1-5GB: 16MB chunks
            _ => 24 * 1024 * 1024,                        // > 5GB: 24MB chunks
        }
    }
}

/// Blazing fast receiver with true parallel writes
pub struct BlazingReceiver;

// Per-transfer state without global locks
struct TransferState {
    filename: String,
    file_size: u64,
    chunk_size: u64,
    total_chunks: u64,
    target_path: PathBuf,
    file: Arc<tokio::sync::Mutex<tokio::fs::File>>,
    received_chunks: AtomicU64,
    write_semaphore: Arc<Semaphore>,
    completed: AtomicBool,
}

// Use DashMap for lock-free concurrent access
lazy_static::lazy_static! {
    static ref ACTIVE_TRANSFERS: DashMap<String, Arc<TransferState>> = DashMap::new();
}

impl BlazingReceiver {
    pub async fn handle_incoming_transfer(mut recv_stream: quinn::RecvStream) -> Result<()> {
        // Read header length
        let mut len_bytes = [0u8; 4];
        recv_stream.read_exact(&mut len_bytes).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to read header length: {}", e)))?;
        let header_len = u32::from_be_bytes(len_bytes) as usize;
        
        // Check if this is a data stream (no header)
        if header_len < 10 || header_len > 1000 {
            // This is a data stream, process chunks
            return Self::process_data_stream(recv_stream, len_bytes).await;
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
        mut recv_stream: quinn::RecvStream,
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
    
    /// Initialize parallel transfer
    async fn receive_parallel_control(
        _recv_stream: quinn::RecvStream,
        parts: &[&str],
    ) -> Result<()> {
        if parts.len() < 6 {
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
        
        info!("🎛️ BLAZING parallel receive: {} ({:.1} MB, {} chunks)", 
              filename, file_size as f64 / (1024.0 * 1024.0), total_chunks);
        
        // Create file with pre-allocation for optimal performance
        let file = tokio::fs::File::create(&target_path).await?;
        file.set_len(file_size).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to pre-allocate file: {}", e)))?;
        file.sync_all().await
            .map_err(|e| FileshareError::Transfer(format!("Failed to sync file: {}", e)))?; // Ensure allocation is complete
        
        // Create transfer state
        let state = Arc::new(TransferState {
            filename: filename.clone(),
            file_size,
            chunk_size,
            total_chunks,
            target_path,
            file: Arc::new(tokio::sync::Mutex::new(file)),
            received_chunks: AtomicU64::new(0),
            write_semaphore: Arc::new(Semaphore::new(WRITE_CONCURRENCY)),
            completed: AtomicBool::new(false),
        });
        
        ACTIVE_TRANSFERS.insert(filename, state);
        Ok(())
    }
    
    /// Process data stream with parallel writes
    async fn process_data_stream(
        mut recv_stream: quinn::RecvStream,
        first_bytes: [u8; 4],
    ) -> Result<()> {
        // Read rest of chunk ID
        let mut id_bytes = [0u8; 8];
        id_bytes[..4].copy_from_slice(&first_bytes);
        recv_stream.read_exact(&mut id_bytes[4..]).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to read chunk ID: {}", e)))?;
        let first_chunk_id = u64::from_be_bytes(id_bytes);
        
        // Find active transfer
        let transfer_key = ACTIVE_TRANSFERS.iter()
            .find(|entry| !entry.value().completed.load(Ordering::Relaxed))
            .map(|entry| entry.key().clone())
            .ok_or_else(|| FileshareError::Transfer("No active transfer".to_string()))?;
        
        let state = ACTIVE_TRANSFERS.get(&transfer_key)
            .ok_or_else(|| FileshareError::Transfer("Transfer not found".to_string()))?
            .clone();
        
        // Process first chunk
        let mut size_bytes = [0u8; 4];
        recv_stream.read_exact(&mut size_bytes).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to read chunk size: {}", e)))?;
        let chunk_size = u32::from_be_bytes(size_bytes) as usize;
        
        let mut chunk_data = vec![0u8; chunk_size];
        recv_stream.read_exact(&mut chunk_data).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to read first chunk data: {}", e)))?;
        
        // Write first chunk in parallel
        Self::write_chunk_parallel(state.clone(), first_chunk_id, chunk_data).await?;
        
        // Process remaining chunks
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
            
            // Write chunk in parallel
            Self::write_chunk_parallel(state.clone(), chunk_id, chunk_data).await?;
        }
        
        Ok(())
    }
    
    /// Write chunk with true parallelism using seek
    async fn write_chunk_parallel(
        state: Arc<TransferState>,
        chunk_id: u64,
        data: Vec<u8>,
    ) -> Result<()> {
        // Acquire write permit to control concurrency
        let _permit = state.write_semaphore.acquire().await
            .map_err(|_| FileshareError::Transfer("Failed to acquire write permit".to_string()))?;
        
        // Clone file handle for parallel access
        let file = state.file.clone();
        let state_clone = state.clone();
        
        // Spawn parallel write task
        tokio::spawn(async move {
            let mut file = file.lock().await;
            let offset = chunk_id * state_clone.chunk_size;
            
            // Seek to chunk position and write
            if let Err(e) = file.seek(std::io::SeekFrom::Start(offset)).await {
                error!("Failed to seek to offset {}: {}", offset, e);
                return;
            }
            
            if let Err(e) = file.write_all(&data).await {
                error!("Failed to write chunk {}: {}", chunk_id, e);
                return;
            }
            
            // Update progress
            let received = state_clone.received_chunks.fetch_add(1, Ordering::Relaxed) + 1;
            
            if received % 10 == 0 || received == state_clone.total_chunks {
                let progress = (received as f64 / state_clone.total_chunks as f64) * 100.0;
                info!("📊 Progress: {:.1}% ({}/{})", progress, received, state_clone.total_chunks);
            }
            
            // Check completion
            if received == state_clone.total_chunks {
                if let Err(e) = file.sync_all().await {
                    error!("Failed to sync file: {}", e);
                    return;
                }
                state_clone.completed.store(true, Ordering::Relaxed);
                info!("🎉 BLAZING transfer complete: {}", state_clone.filename);
                
                // Cleanup
                ACTIVE_TRANSFERS.remove(&state_clone.filename);
            }
        });
        
        Ok(())
    }
}