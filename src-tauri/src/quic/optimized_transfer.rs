use crate::quic::stream_manager::StreamManager;
use crate::{FileshareError, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt, AsyncSeekExt};
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};
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
        
        // Choose transfer method based on file size for true parallelism
        let result = if file_size <= SMALL_FILE_THRESHOLD {
            Self::single_stream_transfer(stream_manager, source_path, target_path, filename, file_size).await
        } else {
            Self::parallel_stream_transfer(stream_manager, source_path, target_path, filename, file_size).await
        };
        
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
    
    /// Parallel stream transfer for large files - true parallelism with proper coordination
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
        
        info!("🚀 Using {} parallel streams for large file ({:.1} MB, {} chunks of {:.1} MB)", 
              stream_count, 
              file_size as f64 / (1024.0 * 1024.0), 
              total_chunks,
              chunk_size as f64 / (1024.0 * 1024.0));
        
        // Step 1: Send control message with transfer metadata
        let control_stream = stream_manager.open_file_transfer_streams(1).await?
            .into_iter().next()
            .ok_or_else(|| FileshareError::Transfer("Failed to create control stream".to_string()))?;
            
        Self::send_parallel_control_message(control_stream, &filename, file_size, &target_path, chunk_size, total_chunks).await?;
        
        // Step 2: Open data streams and distribute chunks
        let data_streams = stream_manager.open_file_transfer_streams(stream_count).await?;
        
        // Step 3: Launch parallel chunk senders
        let mut handles = Vec::new();
        let file_path = Arc::new(source_path);
        
        for (stream_idx, mut stream) in data_streams.into_iter().enumerate() {
            let file_path = file_path.clone();
            let chunks_per_stream = (total_chunks + stream_count as u64 - 1) / stream_count as u64;
            let start_chunk = stream_idx as u64 * chunks_per_stream;
            let end_chunk = std::cmp::min(start_chunk + chunks_per_stream, total_chunks);
            
            if start_chunk >= total_chunks {
                // Close unused streams
                if let Err(e) = stream.finish() {
                    warn!("Failed to close unused stream {}: {}", stream_idx, e);
                }
                continue;
            }
            
            let handle = tokio::spawn(async move {
                Self::send_parallel_chunks(
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
        
        // Step 4: Wait for all streams to complete
        let mut completed = 0;
        for (idx, handle) in handles.into_iter().enumerate() {
            match handle.await {
                Ok(Ok(())) => {
                    completed += 1;
                    debug!("✅ Stream {} completed successfully ({}/{})", idx, completed, stream_count);
                },
                Ok(Err(e)) => {
                    error!("❌ Stream {} failed: {}", idx, e);
                    return Err(e);
                },
                Err(e) => {
                    error!("❌ Stream {} panicked: {}", idx, e);
                    return Err(FileshareError::Transfer(format!("Stream {} panicked: {}", idx, e)));
                }
            }
        }
        
        info!("🎉 All {} parallel streams completed successfully", completed);
        Ok(())
    }
    
    /// Send control message for parallel transfer coordination
    async fn send_parallel_control_message(
        mut control_stream: quinn::SendStream,
        filename: &str,
        file_size: u64,
        target_path: &str,
        chunk_size: u64,
        total_chunks: u64,
    ) -> Result<()> {
        let header = format!("PARALLEL_CONTROL|{}|{}|{}|{}|{}", filename, file_size, target_path, chunk_size, total_chunks);
        let header_bytes = header.as_bytes();
        
        // Send header length and header
        control_stream.write_all(&(header_bytes.len() as u32).to_be_bytes()).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to write control header length: {}", e)))?;
        control_stream.write_all(header_bytes).await
            .map_err(|e| FileshareError::Transfer(format!("Failed to write control header: {}", e)))?;
        
        control_stream.finish()
            .map_err(|e| FileshareError::Transfer(format!("Failed to finish control stream: {}", e)))?;
            
        info!("✅ Parallel control message sent: {} chunks", total_chunks);
        Ok(())
    }
    
    /// Send chunks with proper sequence numbers for parallel assembly
    async fn send_parallel_chunks(
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
        
        debug!("📤 Stream {} starting: chunks {}-{}", stream_idx, start_chunk, end_chunk - 1);
        
        for chunk_idx in start_chunk..end_chunk {
            // Seek to chunk position
            let chunk_offset = chunk_idx * chunk_size;
            file.seek(std::io::SeekFrom::Start(chunk_offset)).await
                .map_err(|e| FileshareError::FileOperation(format!("Failed to seek: {}", e)))?;
            
            // Calculate actual bytes to read for this chunk
            let remaining_bytes = file_size.saturating_sub(chunk_offset);
            let bytes_to_read = std::cmp::min(chunk_size, remaining_bytes) as usize;
            
            // Read chunk data
            let mut total_read = 0;
            while total_read < bytes_to_read {
                match file.read(&mut buffer[total_read..bytes_to_read]).await {
                    Ok(0) => break, // EOF
                    Ok(n) => total_read += n,
                    Err(e) => return Err(FileshareError::FileOperation(format!("Read error: {}", e))),
                }
            }
            
            // Send chunk with sequence number: [chunk_id: u64][size: u32][data: bytes]
            stream.write_all(&chunk_idx.to_be_bytes()).await
                .map_err(|e| FileshareError::Transfer(format!("Failed to write chunk ID: {}", e)))?;
            stream.write_all(&(total_read as u32).to_be_bytes()).await
                .map_err(|e| FileshareError::Transfer(format!("Failed to write chunk size: {}", e)))?;
            stream.write_all(&buffer[..total_read]).await
                .map_err(|e| FileshareError::Transfer(format!("Failed to write chunk data: {}", e)))?;
            
            if chunk_idx % 10 == 0 {
                debug!("📦 Stream {}: Sent chunk {} ({} bytes)", stream_idx, chunk_idx, total_read);
            }
        }
        
        stream.finish()
            .map_err(|e| FileshareError::Transfer(format!("Failed to finish stream: {}", e)))?;
            
        debug!("✅ Stream {} completed: sent {} chunks", stream_idx, end_chunk - start_chunk);
        Ok(())
    }
    
    /// Calculate optimal chunk size based on file size
    fn calculate_optimal_chunk_size(file_size: u64) -> u64 {
        match file_size {
            0..=50_000_000 => 1 * 1024 * 1024,             // <= 50MB: 1MB chunks
            50_000_001..=200_000_000 => 2 * 1024 * 1024,   // 50MB-200MB: 2MB chunks
            200_000_001..=1_000_000_000 => 4 * 1024 * 1024,  // 200MB-1GB: 4MB chunks
            1_000_000_001..=5_000_000_000 => 8 * 1024 * 1024,  // 1GB-5GB: 8MB chunks
            _ => 16 * 1024 * 1024,                         // > 5GB: 16MB chunks
        }
    }
}

/// Receiver side - handles both single and parallel transfers
pub struct OptimizedReceiver;

static PARALLEL_TRANSFERS: std::sync::OnceLock<Arc<Mutex<HashMap<String, ParallelReceiver>>>> = std::sync::OnceLock::new();

#[derive(Debug)]
struct ParallelReceiver {
    filename: String,
    file_size: u64,
    target_path: PathBuf,
    chunk_size: u64,
    total_chunks: u64,
    received_chunks: u64,
    file: Option<tokio::fs::File>,
    chunk_buffer: HashMap<u64, Vec<u8>>, // chunk_id -> data
    next_chunk_to_write: u64,
    completed: bool,
}

impl OptimizedReceiver {
    pub async fn handle_incoming_transfer(mut recv_stream: quinn::RecvStream) -> Result<()> {
        // Try to read header length first
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
                    "PARALLEL_CONTROL" => Self::receive_parallel_control(recv_stream, &parts).await,
                    _ => Err(FileshareError::Transfer("Invalid transfer header".to_string())),
                }
            },
            Err(_) => {
                // This is a parallel data stream - read chunks with sequence numbers
                Self::receive_parallel_data_stream(recv_stream).await
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
    
    async fn receive_parallel_control(_recv_stream: quinn::RecvStream, parts: &[&str]) -> Result<()> {
        if parts.len() != 6 {
            return Err(FileshareError::Transfer("Invalid parallel control header".to_string()));
        }
        
        let filename = parts[1].to_string();
        let file_size: u64 = parts[2].parse()
            .map_err(|e| FileshareError::Transfer(format!("Invalid file size: {}", e)))?;
        let target_path = PathBuf::from(parts[3]);
        let chunk_size: u64 = parts[4].parse()
            .map_err(|e| FileshareError::Transfer(format!("Invalid chunk size: {}", e)))?;
        let total_chunks: u64 = parts[5].parse()
            .map_err(|e| FileshareError::Transfer(format!("Invalid total chunks: {}", e)))?;
        
        info!("🎛️ Parallel transfer control: {} ({:.1} MB, {} chunks of {:.1} MB)", 
              filename, 
              file_size as f64 / (1024.0 * 1024.0), 
              total_chunks,
              chunk_size as f64 / (1024.0 * 1024.0));
        
        // Create target file and directory
        if let Some(parent) = target_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let file = tokio::fs::File::create(&target_path).await?;
        
        // Initialize parallel receiver
        let receiver = ParallelReceiver {
            filename: filename.clone(),
            file_size,
            target_path: target_path.clone(),
            chunk_size,
            total_chunks,
            received_chunks: 0,
            file: Some(file),
            chunk_buffer: HashMap::new(),
            next_chunk_to_write: 0,
            completed: false,
        };
        
        let transfers = PARALLEL_TRANSFERS.get_or_init(|| Arc::new(Mutex::new(HashMap::new())));
        {
            let mut transfers_lock = transfers.lock().await;
            transfers_lock.insert(filename.clone(), receiver);
        }
        
        info!("✅ Parallel receiver initialized for: {}", filename);
        Ok(())
    }
    
    async fn receive_parallel_data_stream(mut recv_stream: quinn::RecvStream) -> Result<()> {
        debug!("📥 Starting parallel data stream reception");
        
        loop {
            // Read chunk header: [chunk_id: u64][size: u32]
            let mut chunk_header = [0u8; 12]; // 8 + 4 bytes
            match recv_stream.read_exact(&mut chunk_header).await {
                Ok(_) => {
                    let chunk_id = u64::from_be_bytes(chunk_header[0..8].try_into().unwrap());
                    let chunk_size = u32::from_be_bytes(chunk_header[8..12].try_into().unwrap()) as usize;
                    
                    // Read chunk data
                    let mut chunk_data = vec![0u8; chunk_size];
                    recv_stream.read_exact(&mut chunk_data).await
                        .map_err(|e| FileshareError::Transfer(format!("Failed to read chunk data: {}", e)))?;
                    
                    // Process the chunk
                    Self::process_received_chunk(chunk_id, chunk_data).await?;
                },
                Err(e) => {
                    // Stream finished or error
                    debug!("📥 Data stream finished: {}", e);
                    break;
                }
            }
        }
        
        debug!("✅ Parallel data stream reception completed");
        Ok(())
    }
    
    async fn process_received_chunk(chunk_id: u64, chunk_data: Vec<u8>) -> Result<()> {
        let transfers = PARALLEL_TRANSFERS.get_or_init(|| Arc::new(Mutex::new(HashMap::new())));
        let mut transfers_lock = transfers.lock().await;
        
        // Find the active transfer (assuming one active transfer for now)
        let filename = transfers_lock.keys().next().cloned()
            .ok_or_else(|| FileshareError::Transfer("No active parallel transfer".to_string()))?;
        
        let receiver = transfers_lock.get_mut(&filename)
            .ok_or_else(|| FileshareError::Transfer("Transfer not found".to_string()))?;
        
        if receiver.completed {
            return Ok(());
        }
        
        debug!("📦 Received chunk {} ({} bytes)", chunk_id, chunk_data.len());
        
        // Store chunk in buffer
        receiver.chunk_buffer.insert(chunk_id, chunk_data);
        receiver.received_chunks += 1;
        
        // Write chunks in order
        while let Some(chunk_data) = receiver.chunk_buffer.remove(&receiver.next_chunk_to_write) {
            if let Some(ref mut file) = receiver.file {
                file.write_all(&chunk_data).await
                    .map_err(|e| FileshareError::FileOperation(format!("Failed to write chunk: {}", e)))?;
            }
            receiver.next_chunk_to_write += 1;
            
            if receiver.next_chunk_to_write % 50 == 0 {
                let progress = (receiver.next_chunk_to_write as f64 / receiver.total_chunks as f64) * 100.0;
                info!("📊 Progress: {:.1}% ({}/{})", progress, receiver.next_chunk_to_write, receiver.total_chunks);
            }
        }
        
        // Check if transfer is complete
        if receiver.next_chunk_to_write >= receiver.total_chunks {
            if let Some(ref mut file) = receiver.file {
                file.flush().await
                    .map_err(|e| FileshareError::FileOperation(format!("Failed to flush file: {}", e)))?;
            }
            receiver.completed = true;
            
            info!("🎉 Parallel transfer completed: {} ({:.1} MB, {} chunks)", 
                  filename, 
                  receiver.file_size as f64 / (1024.0 * 1024.0),
                  receiver.total_chunks);
        }
        
        Ok(())
    }
}