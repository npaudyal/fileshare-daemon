use crate::quic::stream_manager::StreamManager;
use crate::{FileshareError, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt, AsyncSeekExt};
use tokio::sync::Mutex;
use tracing::{debug, error, info};
use uuid::Uuid;

// Optimized constants for maximum performance
const SMALL_FILE_THRESHOLD: u64 = 10 * 1024 * 1024; // 10MB
const OPTIMAL_BUFFER_SIZE: usize = 8 * 1024 * 1024; // 8MB for maximum throughput
const MAX_PARALLEL_STREAMS: usize = 32; // Maximum concurrent streams for large files

pub struct OptimizedTransfer;

impl OptimizedTransfer {
    /// Single entry point for all file transfers - automatically optimizes based on file size
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
        
        info!("🚀 Starting optimized transfer: {} ({:.1} MB)", 
              filename, file_size as f64 / (1024.0 * 1024.0));
        
        // Temporarily use single stream for all transfers to fix immediate issue
        let result = Self::single_stream_transfer(stream_manager, source_path, target_path, filename, file_size).await;
        
        match result {
            Ok(()) => {
                let duration = start_time.elapsed();
                let speed_mbps = (file_size as f64 * 8.0) / (duration.as_secs_f64() * 1_000_000.0);
                info!("✅ Transfer complete: {:.1} MB in {:.2}s ({:.1} Mbps)", 
                      file_size as f64 / (1024.0 * 1024.0), duration.as_secs_f64(), speed_mbps);
            }
            Err(e) => {
                error!("❌ Transfer failed: {}", e);
                return Err(e);
            }
        }
        
        Ok(())
    }
    
    /// Single stream transfer for small files - minimal overhead, maximum speed
    async fn single_stream_transfer(
        stream_manager: Arc<StreamManager>,
        source_path: PathBuf,
        target_path: String,
        filename: String,
        file_size: u64,
    ) -> Result<()> {
        info!("📊 Using single stream transfer ({:.1} MB)", file_size as f64 / (1024.0 * 1024.0));
        
        // Open a single file transfer stream
        let mut stream = stream_manager.open_file_transfer_streams(1).await?
            .into_iter().next()
            .ok_or_else(|| FileshareError::Transfer("Failed to create stream".to_string()))?;
        
        // Send file header
        let header = format!("FILEINFO|{}|{}|{}", filename, file_size, target_path);
        let header_bytes = header.as_bytes();
        stream.write_all(&(header_bytes.len() as u32).to_be_bytes()).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to write header length: {}", e)))?;
        stream.write_all(header_bytes).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to write header: {}", e)))?;
        
        // Open file and transfer with large buffer
        let mut file = tokio::fs::File::open(&source_path).await?;
        let mut buffer = vec![0u8; OPTIMAL_BUFFER_SIZE];
        let mut total_sent = 0u64;
        
        loop {
            match file.read(&mut buffer).await {
                Ok(0) => break, // EOF
                Ok(n) => {
                    stream.write_all(&buffer[0..n]).await
                        .map_err(|e| FileshareError::Transfer(format!("Failed to write data: {}", e)))?;
                    total_sent += n as u64;
                    
                    if total_sent % (50 * 1024 * 1024) == 0 { // Log every 50MB
                        debug!("Progress: {:.1}%", (total_sent as f64 / file_size as f64) * 100.0);
                    }
                }
                Err(e) => return Err(FileshareError::FileOperation(format!("Read error: {}", e))),
            }
        }
        
        stream.finish()
            .map_err(|e| FileshareError::Transfer(format!("Failed to finish stream: {}", e)))?;
        Ok(())
    }
    
    /// Parallel stream transfer for large files - maximum parallelism and throughput
    async fn parallel_stream_transfer(
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
        
        info!("📊 Using {} parallel streams for large file ({:.1} MB, {} chunks of {:.1} MB)", 
              stream_count, 
              file_size as f64 / (1024.0 * 1024.0), 
              total_chunks,
              chunk_size as f64 / (1024.0 * 1024.0));
        
        // Open multiple streams
        let mut streams = stream_manager.open_file_transfer_streams(stream_count).await?;
        
        // Send file info on first stream only
        {
            let first_stream = &mut streams[0];
            let header = format!("PARALLEL|{}|{}|{}|{}|{}", filename, file_size, target_path, chunk_size, total_chunks);
            let header_bytes = header.as_bytes();
            first_stream.write_all(&(header_bytes.len() as u32).to_be_bytes()).await
                .map_err(|e| FileshareError::Transfer(format!("Failed to write parallel header length: {}", e)))?;
            first_stream.write_all(header_bytes).await
                .map_err(|e| FileshareError::Transfer(format!("Failed to write parallel header: {}", e)))?;
        }
        
        // Distribute chunks across streams
        let mut handles = Vec::new();
        let file_path = Arc::new(source_path);
        
        for (stream_idx, mut stream) in streams.into_iter().enumerate() {
            let file_path = file_path.clone();
            let chunks_per_stream = (total_chunks + stream_count as u64 - 1) / stream_count as u64;
            let start_chunk = stream_idx as u64 * chunks_per_stream;
            let end_chunk = std::cmp::min(start_chunk + chunks_per_stream, total_chunks);
            
            if start_chunk >= total_chunks {
                stream.finish()
            .map_err(|e| FileshareError::Transfer(format!("Failed to finish stream: {}", e)))?;
                continue;
            }
            
            let handle = tokio::spawn(async move {
                Self::send_chunks_range(
                    stream,
                    file_path,
                    start_chunk,
                    end_chunk,
                    chunk_size,
                    file_size,
                    stream_idx,
                ).await
            });
            
            handles.push(handle);
        }
        
        // Wait for all streams to complete
        for (idx, handle) in handles.into_iter().enumerate() {
            match handle.await {
                Ok(Ok(())) => debug!("Stream {} completed successfully", idx),
                Ok(Err(e)) => return Err(e),
                Err(e) => return Err(FileshareError::Transfer(format!("Stream {} panicked: {}", idx, e))),
            }
        }
        
        Ok(())
    }
    
    /// Send a range of chunks on a single stream (simplified - no chunk headers)
    async fn send_chunks_range(
        mut stream: quinn::SendStream,
        file_path: Arc<PathBuf>,
        start_chunk: u64,
        end_chunk: u64,
        chunk_size: u64,
        file_size: u64,
        stream_idx: usize,
    ) -> Result<()> {
        let mut file = tokio::fs::File::open(file_path.as_ref()).await?;
        let mut buffer = vec![0u8; chunk_size as usize];
        
        for chunk_idx in start_chunk..end_chunk {
            // Seek to chunk position
            let chunk_offset = chunk_idx * chunk_size;
            file.seek(std::io::SeekFrom::Start(chunk_offset)).await
                .map_err(|e| FileshareError::FileOperation(format!("Failed to seek: {}", e)))?;
            
            // Calculate actual bytes to read for this chunk
            let remaining_bytes = file_size.saturating_sub(chunk_offset);
            let bytes_to_read = std::cmp::min(chunk_size, remaining_bytes) as usize;
            
            // Read chunk
            let mut total_read = 0;
            while total_read < bytes_to_read {
                match file.read(&mut buffer[total_read..bytes_to_read]).await {
                    Ok(0) => break, // Unexpected EOF
                    Ok(n) => total_read += n,
                    Err(e) => return Err(FileshareError::FileOperation(format!("Read error: {}", e))),
                }
            }
            
            // Send chunk data directly (no headers - order handled by stream ID)
            stream.write_all(&buffer[..total_read]).await
                .map_err(|e| FileshareError::Transfer(format!("Failed to write chunk data: {}", e)))?;
            
            if chunk_idx % 10 == 0 {
                debug!("Stream {}: Sent chunk {}/{}", stream_idx, chunk_idx - start_chunk + 1, end_chunk - start_chunk);
            }
        }
        
        stream.finish()
            .map_err(|e| FileshareError::Transfer(format!("Failed to finish stream: {}", e)))?;
        Ok(())
    }
    
    /// Calculate optimal chunk size based on file size
    fn calculate_optimal_chunk_size(file_size: u64) -> u64 {
        match file_size {
            0..=100_000_000 => 2 * 1024 * 1024,           // <= 100MB: 2MB chunks
            100_000_001..=1_000_000_000 => 4 * 1024 * 1024,  // 100MB-1GB: 4MB chunks
            1_000_000_001..=10_000_000_000 => 8 * 1024 * 1024,  // 1GB-10GB: 8MB chunks
            _ => 16 * 1024 * 1024,                         // > 10GB: 16MB chunks
        }
    }
}

/// Receiver side - handles both single and parallel transfers
pub struct OptimizedReceiver;

static PARALLEL_TRANSFERS: std::sync::OnceLock<Arc<Mutex<HashMap<String, ParallelTransfer>>>> = std::sync::OnceLock::new();

#[derive(Debug)]
struct ParallelTransfer {
    filename: String,
    file_size: u64,
    target_path: PathBuf,
    chunk_size: u64,
    total_chunks: u64,
    received_chunks: u64,
    file: Option<tokio::fs::File>,
    chunks_received: std::collections::HashSet<u64>,
}

impl OptimizedReceiver {
    pub async fn handle_incoming_transfer(mut recv_stream: quinn::RecvStream) -> Result<()> {
        // Try to read header - if it fails, this might be a parallel chunk stream
        let mut len_bytes = [0u8; 4];
        match recv_stream.read_exact(&mut len_bytes).await {
            Ok(_) => {
                let header_len = u32::from_be_bytes(len_bytes) as usize;
                
                // Read header
                let mut header_bytes = vec![0u8; header_len];
                recv_stream.read_exact(&mut header_bytes).await
                    .map_err(|e| FileshareError::Transfer(format!("Failed to read header: {}", e)))?;
                let header = String::from_utf8(header_bytes)
                    .map_err(|e| FileshareError::Transfer(format!("Invalid UTF8 in header: {}", e)))?;
                
                let parts: Vec<&str> = header.split('|').collect();
                match parts[0] {
                    "FILEINFO" => Self::receive_single_stream(recv_stream, &parts).await,
                    "PARALLEL" => Self::receive_parallel_header(recv_stream, &parts).await,
                    _ => Err(FileshareError::Transfer("Invalid transfer header".to_string())),
                }
            },
            Err(_) => {
                // This is likely a parallel chunk stream - handle as raw data
                Self::receive_parallel_chunk(recv_stream).await
            }
        }
    }
    
    async fn receive_single_stream(mut recv_stream: quinn::RecvStream, parts: &[&str]) -> Result<()> {
        if parts.len() != 4 {
            return Err(FileshareError::Transfer("Invalid single stream header".to_string()));
        }
        
        let filename = parts[1];
        let file_size: u64 = parts[2].parse()
            .map_err(|e| FileshareError::Transfer(format!("Invalid file size: {}", e)))?;
        let target_path = parts[3];
        
        info!("📥 Receiving file: {} ({:.1} MB) -> {}", 
              filename, file_size as f64 / (1024.0 * 1024.0), target_path);
        
        // Create target file
        let target_file_path = PathBuf::from(target_path);
        if let Some(parent) = target_file_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        
        let mut file = tokio::fs::File::create(&target_file_path).await?;
        let mut buffer = vec![0u8; OPTIMAL_BUFFER_SIZE];
        let mut bytes_received = 0u64;
        
        // Receive file data
        loop {
            match recv_stream.read(&mut buffer).await {
                Ok(Some(0)) | Ok(None) => break,
                Ok(Some(n)) => {
                    file.write_all(&buffer[0..n]).await
                        .map_err(|e| FileshareError::FileOperation(format!("Failed to write file: {}", e)))?;
                    bytes_received += n as u64;
                    
                    if bytes_received % (50 * 1024 * 1024) == 0 {
                        debug!("Progress: {:.1}%", (bytes_received as f64 / file_size as f64) * 100.0);
                    }
                }
                Err(e) => return Err(FileshareError::Transfer(format!("Receive error: {}", e))),
            }
        }
        
        file.flush().await
            .map_err(|e| FileshareError::FileOperation(format!("Failed to flush file: {}", e)))?;
        info!("✅ File received successfully: {} bytes", bytes_received);
        Ok(())
    }
    
    async fn receive_parallel_header(mut recv_stream: quinn::RecvStream, parts: &[&str]) -> Result<()> {
        if parts.len() != 6 {
            return Err(FileshareError::Transfer("Invalid parallel header".to_string()));
        }
        
        let filename = parts[1].to_string();
        let file_size: u64 = parts[2].parse()
            .map_err(|e| FileshareError::Transfer(format!("Invalid file size: {}", e)))?;
        let target_path = PathBuf::from(parts[3]);
        let chunk_size: u64 = parts[4].parse()
            .map_err(|e| FileshareError::Transfer(format!("Invalid chunk size: {}", e)))?;
        let total_chunks: u64 = parts[5].parse()
            .map_err(|e| FileshareError::Transfer(format!("Invalid total chunks: {}", e)))?;
        
        info!("📥 Parallel transfer header: {} ({:.1} MB, {} chunks)", 
              filename, file_size as f64 / (1024.0 * 1024.0), total_chunks);
        
        // Create target file
        if let Some(parent) = target_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let file = tokio::fs::File::create(&target_path).await?;
        
        // Initialize parallel transfer tracking
        let transfer = ParallelTransfer {
            filename: filename.clone(),
            file_size,
            target_path: target_path.clone(),
            chunk_size,
            total_chunks,
            received_chunks: 0,
            file: Some(file),
            chunks_received: std::collections::HashSet::new(),
        };
        
        let transfers = PARALLEL_TRANSFERS.get_or_init(|| Arc::new(Mutex::new(HashMap::new())));
        {
            let mut transfers_lock = transfers.lock().await;
            transfers_lock.insert(filename.clone(), transfer);
        }
        
        // Continue reading remaining data from this stream (it's also a chunk stream)
        Self::receive_parallel_chunk_with_key(recv_stream, filename).await
    }
    
    async fn receive_parallel_chunk(recv_stream: quinn::RecvStream) -> Result<()> {
        // For chunk streams without filename, try to find the active transfer
        let transfers = PARALLEL_TRANSFERS.get_or_init(|| Arc::new(Mutex::new(HashMap::new())));
        let filename = {
            let transfers_lock = transfers.lock().await;
            transfers_lock.keys().next().cloned()
        };
        
        if let Some(filename) = filename {
            Self::receive_parallel_chunk_with_key(recv_stream, filename).await
        } else {
            Err(FileshareError::Transfer("No active parallel transfer found".to_string()))
        }
    }
    
    async fn receive_parallel_chunk_with_key(mut recv_stream: quinn::RecvStream, filename: String) -> Result<()> {
        let mut buffer = vec![0u8; 8 * 1024 * 1024]; // 8MB buffer
        let mut total_received = 0;
        
        // Read all data from this stream
        loop {
            match recv_stream.read(&mut buffer).await {
                Ok(Some(0)) | Ok(None) => break,
                Ok(Some(n)) => {
                    total_received += n;
                    
                    // Write to file (simplified - just append for now)
                    let transfers = PARALLEL_TRANSFERS.get_or_init(|| Arc::new(Mutex::new(HashMap::new())));
                    let mut transfers_lock = transfers.lock().await;
                    
                    if let Some(transfer) = transfers_lock.get_mut(&filename) {
                        if let Some(ref mut file) = transfer.file {
                            file.write_all(&buffer[..n]).await
                                .map_err(|e| FileshareError::FileOperation(format!("Failed to write: {}", e)))?;
                        }
                        
                        transfer.received_chunks += 1;
                        if transfer.received_chunks >= transfer.total_chunks {
                            info!("✅ Parallel transfer complete: {} ({} bytes)", filename, total_received);
                            if let Some(ref mut file) = transfer.file {
                                file.flush().await
                                    .map_err(|e| FileshareError::FileOperation(format!("Failed to flush: {}", e)))?;
                            }
                        }
                    }
                },
                Err(e) => return Err(FileshareError::Transfer(format!("Receive error: {}", e))),
            }
        }
        
        debug!("Parallel chunk stream complete: {} bytes", total_received);
        Ok(())
    }
}