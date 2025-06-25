use crate::quic::stream_manager::StreamManager;
use crate::{FileshareError, Result};
use dashmap::DashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicBool, Ordering};
use std::time::{Instant, Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt, AsyncSeekExt};
use tokio::sync::{Semaphore, RwLock, mpsc};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

// Optimized constants for maximum blazing performance
const SMALL_FILE_THRESHOLD: u64 = 10 * 1024 * 1024; // 10MB
const OPTIMAL_CHUNK_SIZE: u64 = 8 * 1024 * 1024; // 8MB for better parallelism
const MAX_PARALLEL_STREAMS: usize = 32; // Optimal for most networks
const WRITE_CONCURRENCY: usize = 64; // High concurrent disk writes

pub struct BlazingTransfer;

impl BlazingTransfer {
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
        
        info!("🚀 Starting BLAZING transfer: {} ({:.1} MB)", 
              filename, file_size as f64 / (1024.0 * 1024.0));
        
        // Skip bandwidth probing for instant start
        let bandwidth_mbps = 1000.0; // Assume high LAN bandwidth
        
        let result = if file_size <= SMALL_FILE_THRESHOLD {
            Self::single_stream_transfer(stream_manager, source_path, target_path, filename, file_size).await
        } else {
            Self::blazing_parallel_transfer(
                stream_manager, source_path, target_path, filename, file_size
            ).await
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
        
        let header = format!("BLAZING_SINGLE|{}|{}|{}", filename, file_size, target_path);
        let header_bytes = header.as_bytes();
        stream.write_all(&(header_bytes.len() as u32).to_be_bytes()).await?;
        stream.write_all(header_bytes).await?;
        
        let mut file = tokio::fs::File::open(&source_path).await?;
        let mut buffer = vec![0u8; OPTIMAL_CHUNK_SIZE as usize];
        
        loop {
            match file.read(&mut buffer).await {
                Ok(0) => break,
                Ok(n) => {
                    stream.write_all(&buffer[0..n]).await?;
                }
                Err(e) => return Err(FileshareError::FileOperation(format!("Read error: {}", e))),
            }
        }
        
        stream.finish()?;
        Ok(())
    }
    
    async fn blazing_parallel_transfer(
        stream_manager: Arc<StreamManager>,
        source_path: PathBuf,
        target_path: String,
        filename: String,
        file_size: u64,
    ) -> Result<()> {
        let chunk_size = Self::calculate_optimal_chunk_size(file_size);
        let total_chunks = (file_size + chunk_size - 1) / chunk_size;
        let optimal_streams = Self::calculate_optimal_streams(file_size, total_chunks);
        
        info!("🚀 BLAZING parallel transfer: {} streams, {} chunks of {:.1} MB each", 
              optimal_streams, total_chunks, chunk_size as f64 / (1024.0 * 1024.0));
        
        // Send control message first
        let control_stream = stream_manager.open_file_transfer_streams(1).await?
            .into_iter().next()
            .ok_or_else(|| FileshareError::Transfer("Failed to create control stream".to_string()))?;
            
        Self::send_control_message(
            control_stream, &filename, file_size, &target_path, chunk_size, total_chunks
        ).await?;
        
        info!("✅ Control message sent for {} chunks", total_chunks);
        
        // Small delay to ensure control message is processed first
        tokio::time::sleep(Duration::from_millis(50)).await;
        
        // Create ALL streams immediately - no progressive launch
        let streams = stream_manager.open_file_transfer_streams(optimal_streams).await?;
        
        // Create progress tracking
        let bytes_sent = Arc::new(AtomicU64::new(0));
        let chunks_sent = Arc::new(AtomicU64::new(0));
        
        // Start progress monitor
        let monitor_handle = tokio::spawn(Self::monitor_progress(
            bytes_sent.clone(),
            chunks_sent.clone(),
            file_size,
            total_chunks,
            Instant::now(),
        ));
        
        // Launch all streams immediately with chunks distributed evenly
        let mut handles = Vec::new();
        let chunks_per_stream = (total_chunks + optimal_streams as u64 - 1) / optimal_streams as u64;
        
        for (stream_idx, stream) in streams.into_iter().enumerate() {
            let start_chunk = stream_idx as u64 * chunks_per_stream;
            let end_chunk = ((stream_idx + 1) as u64 * chunks_per_stream).min(total_chunks);
            
            if start_chunk >= total_chunks {
                break;
            }
            
            let file_path = source_path.clone();
            let bytes_sent = bytes_sent.clone();
            let chunks_sent = chunks_sent.clone();
            
            let handle = tokio::spawn(async move {
                Self::blazing_stream_sender(
                    stream,
                    stream_idx,
                    file_path,
                    chunk_size,
                    file_size,
                    start_chunk,
                    end_chunk,
                    bytes_sent,
                    chunks_sent,
                ).await
            });
            
            handles.push(handle);
        }
        
        // Wait for all streams to complete
        for handle in handles {
            handle.await??;
        }
        
        monitor_handle.abort();
        Ok(())
    }
    
    async fn blazing_stream_sender(
        mut stream: quinn::SendStream,
        stream_idx: usize,
        file_path: PathBuf,
        chunk_size: u64,
        file_size: u64,
        start_chunk: u64,
        end_chunk: u64,
        bytes_sent: Arc<AtomicU64>,
        chunks_sent: Arc<AtomicU64>,
    ) -> Result<()> {
        let mut file = tokio::fs::File::open(&file_path).await?;
        let mut buffer = vec![0u8; chunk_size as usize];
        
        // Send filename first to identify this transfer
        let filename = file_path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();
        let filename_bytes = filename.as_bytes();
        
        // Send filename length and filename
        stream.write_all(&(filename_bytes.len() as u32).to_be_bytes()).await?;
        stream.write_all(filename_bytes).await?;
        
        debug!("📤 Stream {} sending chunks {}-{} for file: {}", stream_idx, start_chunk, end_chunk - 1, filename);
        
        for chunk_idx in start_chunk..end_chunk {
            let chunk_offset = chunk_idx * chunk_size;
            file.seek(std::io::SeekFrom::Start(chunk_offset)).await?;
            
            let remaining = file_size.saturating_sub(chunk_offset);
            let bytes_to_read = std::cmp::min(chunk_size, remaining) as usize;
            
            file.read_exact(&mut buffer[..bytes_to_read]).await?;
            
            // Send: [chunk_id: u64][size: u32][data]
            stream.write_all(&chunk_idx.to_be_bytes()).await?;
            stream.write_all(&(bytes_to_read as u32).to_be_bytes()).await?;
            stream.write_all(&buffer[..bytes_to_read]).await?;
            
            bytes_sent.fetch_add(bytes_to_read as u64, Ordering::Relaxed);
            chunks_sent.fetch_add(1, Ordering::Relaxed);
        }
        
        stream.finish()?;
        debug!("✅ Stream {} completed: chunks {}-{}", stream_idx, start_chunk, end_chunk - 1);
        Ok(())
    }
    
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
        
        stream.write_all(&(header_bytes.len() as u32).to_be_bytes()).await?;
        stream.write_all(header_bytes).await?;
        stream.finish()?;
        
        info!("✅ Control message sent for {} chunks", total_chunks);
        Ok(())
    }
    
    async fn monitor_progress(
        bytes_sent: Arc<AtomicU64>,
        chunks_sent: Arc<AtomicU64>,
        file_size: u64,
        total_chunks: u64,
        start_time: Instant,
    ) {
        let mut last_bytes = 0u64;
        let mut last_time = Instant::now();
        
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            
            let current_bytes = bytes_sent.load(Ordering::Relaxed);
            let current_chunks = chunks_sent.load(Ordering::Relaxed);
            
            let interval_bytes = current_bytes - last_bytes;
            let interval_time = last_time.elapsed();
            let interval_speed_mbps = (interval_bytes as f64 * 8.0) / 
                (interval_time.as_secs_f64() * 1_000_000.0);
            
            let progress = (current_chunks as f64 / total_chunks as f64) * 100.0;
            
            if current_bytes < file_size {
                info!("📊 Progress: {:.1}% - Speed: {:.1} Mbps", progress, interval_speed_mbps);
            }
            
            last_bytes = current_bytes;
            last_time = Instant::now();
            
            if current_bytes >= file_size {
                break;
            }
        }
    }
    
    fn calculate_optimal_chunk_size(file_size: u64) -> u64 {
        match file_size {
            0..=100_000_000 => 4 * 1024 * 1024,      // 4MB for small files
            100_000_001..=500_000_000 => 8 * 1024 * 1024,    // 8MB for medium files
            500_000_001..=2_000_000_000 => 16 * 1024 * 1024, // 16MB for large files
            _ => 32 * 1024 * 1024,                            // 32MB for very large files
        }
    }
    
    fn calculate_optimal_streams(file_size: u64, total_chunks: u64) -> usize {
        let base_streams = match file_size {
            0..=50_000_000 => 4,                     // 4 streams for small files
            50_000_001..=200_000_000 => 8,           // 8 streams for medium files  
            200_000_001..=1_000_000_000 => 16,       // 16 streams for large files
            _ => 24,                                  // 24 streams for very large files
        };
        
        base_streams.min(MAX_PARALLEL_STREAMS).min(total_chunks as usize)
    }
}

/// Blazing fast receiver with true parallel writes
pub struct BlazingReceiver;

struct TransferState {
    filename: String,
    file_size: u64,
    chunk_size: u64,
    total_chunks: u64,
    target_path: PathBuf,
    received_chunks: AtomicU64,
    write_semaphore: Arc<Semaphore>,
    completed: AtomicBool,
    chunk_writer: mpsc::Sender<(u64, Vec<u8>)>,
}

lazy_static::lazy_static! {
    static ref ACTIVE_TRANSFERS: DashMap<String, Arc<TransferState>> = DashMap::new();
}

impl BlazingReceiver {
    pub async fn handle_incoming_transfer(mut recv_stream: quinn::RecvStream) -> Result<()> {
        let mut len_bytes = [0u8; 4];
        recv_stream.read_exact(&mut len_bytes).await?;
        let first_len = u32::from_be_bytes(len_bytes) as usize;
        
        debug!("📥 Received stream with first length: {}", first_len);
        
        // Try to read as control message header first
        if first_len >= 10 && first_len <= 1000 {
            let mut header_bytes = vec![0u8; first_len];
            match recv_stream.read_exact(&mut header_bytes).await {
                Ok(_) => {
                    if let Ok(header) = String::from_utf8(header_bytes) {
                        debug!("📥 Potential control header: {}", header);
                        let parts: Vec<&str> = header.split('|').collect();
                        
                        match parts.get(0) {
                            Some(&"BLAZING_SINGLE") => {
                                debug!("📥 Processing as BLAZING_SINGLE control message");
                                return Self::receive_single_stream(recv_stream, &parts).await;
                            },
                            Some(&"BLAZING_PARALLEL") => {
                                debug!("📥 Processing as BLAZING_PARALLEL control message");
                                return Self::receive_parallel_control(recv_stream, &parts).await;
                            },
                            _ => {
                                debug!("📥 Not a control message, treating as data stream");
                                // Not a control message, fall through to data stream handling
                            }
                        }
                    } else {
                        debug!("📥 Failed to parse as UTF-8, treating as data stream");
                    }
                }
                Err(e) => {
                    debug!("📥 Failed to read header bytes: {}, treating as data stream", e);
                    // Not a control message, fall through to data stream handling
                }
            }
        } else {
            debug!("📥 Length {} not in control range, treating as data stream", first_len);
        }
        
        // This is a data stream - the first_len is actually chunk data, not filename length
        let first_bytes = (first_len as u32).to_be_bytes();
        debug!("📥 Processing as legacy data stream (first 4 bytes: {:?})", first_bytes);
        Self::process_data_stream_legacy_direct(recv_stream, first_bytes).await
    }
    
    async fn receive_single_stream(
        mut recv_stream: quinn::RecvStream,
        parts: &[&str],
    ) -> Result<()> {
        if parts.len() < 4 {
            return Err(FileshareError::Transfer("Invalid single transfer header".to_string()));
        }
        
        let filename = parts[1];
        let file_size: u64 = parts[2].parse()?;
        let target_path = PathBuf::from(parts[3]);
        
        info!("📥 Receiving single stream: {} ({:.1} MB)", filename, file_size as f64 / (1024.0 * 1024.0));
        
        let mut file = tokio::fs::File::create(&target_path).await?;
        let mut buffer = vec![0u8; OPTIMAL_CHUNK_SIZE as usize];
        let mut total_received = 0u64;
        
        while total_received < file_size {
            match recv_stream.read(&mut buffer).await {
                Ok(Some(n)) => {
                    file.write_all(&buffer[..n]).await?;
                    total_received += n as u64;
                }
                Ok(None) => break,
                Err(e) => return Err(FileshareError::Transfer(format!("Read error: {}", e))),
            }
        }
        
        file.sync_all().await?;
        info!("✅ Single stream transfer complete: {}", filename);
        Ok(())
    }
    
    async fn receive_parallel_control(
        _recv_stream: quinn::RecvStream,
        parts: &[&str],
    ) -> Result<()> {
        if parts.len() < 6 {
            return Err(FileshareError::Transfer("Invalid parallel header".to_string()));
        }
        
        let filename = parts[1].to_string();
        let file_size: u64 = parts[2].parse()?;
        let target_path = PathBuf::from(parts[3]);
        let chunk_size: u64 = parts[4].parse()?;
        let total_chunks: u64 = parts[5].parse()?;
        
        info!("🎛️ BLAZING parallel receive: {} ({:.1} MB, {} chunks)", 
              filename, file_size as f64 / (1024.0 * 1024.0), total_chunks);
        
        // Create target directory
        if let Some(parent) = target_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        
        // Create file
        let file = tokio::fs::File::create(&target_path).await?;
        
        // Pre-allocate file space
        #[cfg(target_os = "macos")]
        {
            file.set_len(file_size).await?;
        }
        
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            let file_clone = file.try_clone().await?;
            tokio::task::spawn_blocking(move || {
                use std::os::unix::io::AsRawFd;
                let fd = file_clone.as_raw_fd();
                unsafe {
                    libc::fallocate(fd, 0, 0, file_size as i64);
                }
            }).await?;
        }
        
        #[cfg(not(unix))]
        {
            file.set_len(file_size).await?;
        }
        
        // Create chunk writer channel
        let (chunk_tx, mut chunk_rx) = mpsc::channel::<(u64, Vec<u8>)>(100);
        
        // Create transfer state BEFORE spawning writer task
        let state = Arc::new(TransferState {
            filename: filename.clone(),
            file_size,
            chunk_size,
            total_chunks,
            target_path: target_path.clone(),
            received_chunks: AtomicU64::new(0),
            write_semaphore: Arc::new(Semaphore::new(WRITE_CONCURRENCY)),
            completed: AtomicBool::new(false),
            chunk_writer: chunk_tx,
        });
        
        // Insert into ACTIVE_TRANSFERS immediately - this must happen BEFORE spawning tasks
        ACTIVE_TRANSFERS.insert(filename.clone(), state.clone());
        info!("✅ Transfer state created for: {}", filename);
        
        // Small delay to ensure transfer state is fully registered
        tokio::time::sleep(Duration::from_millis(10)).await;
        
        // Now spawn writer task
        let target_path_clone = target_path.clone();
        let filename_clone = filename.clone();
        let writer_task = tokio::spawn(async move {
            let mut file = tokio::fs::OpenOptions::new()
                .write(true)
                .open(&target_path_clone)
                .await?;
            
            let mut next_chunk = 0u64;
            let mut buffer: std::collections::HashMap<u64, Vec<u8>> = std::collections::HashMap::new();
            
            while next_chunk < total_chunks {
                if let Some(data) = buffer.remove(&next_chunk) {
                    file.write_all(&data).await?;
                    next_chunk += 1;
                } else if let Some((chunk_id, data)) = chunk_rx.recv().await {
                    if chunk_id == next_chunk {
                        file.write_all(&data).await?;
                        next_chunk += 1;
                    } else {
                        buffer.insert(chunk_id, data);
                    }
                }
                
                if next_chunk % 50 == 0 {
                    let progress = (next_chunk as f64 / total_chunks as f64) * 100.0;
                    info!("📊 Write progress: {:.1}% ({}/{})", progress, next_chunk, total_chunks);
                }
            }
            
            file.sync_all().await?;
            info!("✅ File write completed: {}", filename_clone);
            Ok::<(), FileshareError>(())
        });
        
        // Monitor writer task
        tokio::spawn(async move {
            match writer_task.await {
                Ok(Ok(())) => {
                    ACTIVE_TRANSFERS.remove(&filename);
                }
                Ok(Err(e)) => {
                    error!("Writer task failed: {}", e);
                    ACTIVE_TRANSFERS.remove(&filename);
                }
                Err(e) => {
                    error!("Writer task panicked: {}", e);
                    ACTIVE_TRANSFERS.remove(&filename);
                }
            }
        });
        
        Ok(())
    }
    
    async fn process_data_stream_legacy_direct(
        mut recv_stream: quinn::RecvStream,
        first_bytes: [u8; 4],
    ) -> Result<()> {
        debug!("📥 Processing legacy data stream with first bytes: {:?}", first_bytes);
        
        // Wait for an active transfer to be available
        let start_time = Instant::now();
        let (transfer_key, state) = loop {
            if let Some(entry) = ACTIVE_TRANSFERS.iter()
                .find(|entry| !entry.value().completed.load(Ordering::Relaxed)) {
                break (entry.key().clone(), entry.value().clone());
            }
            
            if start_time.elapsed() > Duration::from_secs(10) {
                return Err(FileshareError::Transfer("No active transfer found within timeout".to_string()));
            }
            
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        
        debug!("📥 Found active transfer: {}", transfer_key);
        
        // The first_bytes are the first 4 bytes of chunk_id
        let mut id_bytes = [0u8; 8];
        id_bytes[..4].copy_from_slice(&first_bytes);
        recv_stream.read_exact(&mut id_bytes[4..]).await?;
        let chunk_id = u64::from_be_bytes(id_bytes);
        
        debug!("📥 Processing chunk {} for transfer: {}", chunk_id, transfer_key);
        
        // Read chunk size
        let mut size_bytes = [0u8; 4];
        recv_stream.read_exact(&mut size_bytes).await?;
        let chunk_size = u32::from_be_bytes(size_bytes) as usize;
        
        if chunk_size > 100 * 1024 * 1024 {
            return Err(FileshareError::Transfer(format!("Chunk too large: {} bytes", chunk_size)));
        }
        
        // Read chunk data
        let mut chunk_data = vec![0u8; chunk_size];
        recv_stream.read_exact(&mut chunk_data).await?;
        
        // Send to writer
        state.chunk_writer.send((chunk_id, chunk_data)).await?;
        
        let received = state.received_chunks.fetch_add(1, Ordering::Relaxed) + 1;
        
        if received % 10 == 0 || received == state.total_chunks {
            let progress = (received as f64 / state.total_chunks as f64) * 100.0;
            debug!("📊 Progress: {:.1}% ({}/{})", progress, received, state.total_chunks);
        }
        
        if received == state.total_chunks {
            state.completed.store(true, Ordering::Relaxed);
            info!("🎉 All chunks received for: {}", state.filename);
        }
        
        Ok(())
    }
    
    async fn process_data_stream_with_filename_direct(
        mut recv_stream: quinn::RecvStream,
        filename_len: usize,
    ) -> Result<()> {
        // Read filename
        if filename_len == 0 || filename_len > 255 {
            debug!("📥 Invalid filename length: {}, treating as legacy data stream", filename_len);
            return Err(FileshareError::Transfer(format!("Invalid filename length: {}", filename_len)));
        }
        
        let mut filename_bytes = vec![0u8; filename_len];
        recv_stream.read_exact(&mut filename_bytes).await?;
        
        // Debug raw bytes
        debug!("📥 Raw filename bytes: {:?}", &filename_bytes[..std::cmp::min(20, filename_bytes.len())]);
        
        let filename = match String::from_utf8(filename_bytes.clone()) {
            Ok(name) => name,
            Err(_) => {
                debug!("📥 Failed to decode filename from {} bytes: {:?}", filename_len, &filename_bytes[..std::cmp::min(10, filename_bytes.len())]);
                return Err(FileshareError::Transfer("Invalid filename encoding".to_string()));
            }
        };
        
        debug!("📥 Processing data stream for file: {}", filename);
        
        // Wait for transfer state to be ready (with timeout)
        let start_time = Instant::now();
        let state = loop {
            if let Some(state) = ACTIVE_TRANSFERS.get(&filename) {
                break state.clone();
            }
            
            if start_time.elapsed() > Duration::from_secs(10) {
                return Err(FileshareError::Transfer(format!("Transfer state not found for: {}", filename)));
            }
            
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        
        // Process chunks
        loop {
            let mut chunk_header = [0u8; 12]; // chunk_id (8) + size (4)
            match recv_stream.read_exact(&mut chunk_header).await {
                Ok(_) => {},
                Err(_) => break,
            }
            
            let chunk_id = u64::from_be_bytes(chunk_header[0..8].try_into().unwrap());
            let chunk_size = u32::from_be_bytes(chunk_header[8..12].try_into().unwrap()) as usize;
            
            if chunk_size > 100 * 1024 * 1024 {
                return Err(FileshareError::Transfer(format!("Chunk too large: {} bytes", chunk_size)));
            }
            
            let mut chunk_data = vec![0u8; chunk_size];
            recv_stream.read_exact(&mut chunk_data).await?;
            
            state.chunk_writer.send((chunk_id, chunk_data)).await?;
            
            let received = state.received_chunks.fetch_add(1, Ordering::Relaxed) + 1;
            
            if received % 10 == 0 || received == state.total_chunks {
                let progress = (received as f64 / state.total_chunks as f64) * 100.0;
                debug!("📊 Progress: {:.1}% ({}/{})", progress, received, state.total_chunks);
            }
        }
        
        Ok(())
    }
    
    async fn process_data_stream(
        mut recv_stream: quinn::RecvStream,
        first_bytes: [u8; 4],
    ) -> Result<()> {
        // Check if this is a filename header (new protocol)
        let potential_filename_len = u32::from_be_bytes(first_bytes) as usize;
        
        if potential_filename_len > 0 && potential_filename_len < 256 {
            // Try to read as filename
            let mut filename_bytes = vec![0u8; potential_filename_len];
            match recv_stream.read_exact(&mut filename_bytes).await {
                Ok(_) => {
                    if let Ok(filename) = String::from_utf8(filename_bytes) {
                        // New protocol with filename header
                        return Self::process_data_stream_with_filename(recv_stream, &filename).await;
                    }
                }
                Err(_) => {}
            }
        }
        
        // Legacy protocol - first_bytes are part of chunk_id
        Self::process_data_stream_legacy(recv_stream, first_bytes).await
    }
    
    async fn process_data_stream_with_filename(
        mut recv_stream: quinn::RecvStream,
        filename: &str,
    ) -> Result<()> {
        debug!("📥 Processing data stream for file: {}", filename);
        
        // Wait for transfer state to be ready (with timeout)
        let start_time = Instant::now();
        let state = loop {
            if let Some(state) = ACTIVE_TRANSFERS.get(filename) {
                break state.clone();
            }
            
            if start_time.elapsed() > Duration::from_secs(5) {
                return Err(FileshareError::Transfer(format!("Transfer state not found for: {}", filename)));
            }
            
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        
        // Process chunks
        loop {
            let mut chunk_header = [0u8; 12]; // chunk_id (8) + size (4)
            match recv_stream.read_exact(&mut chunk_header).await {
                Ok(_) => {},
                Err(_) => break,
            }
            
            let chunk_id = u64::from_be_bytes(chunk_header[0..8].try_into().unwrap());
            let chunk_size = u32::from_be_bytes(chunk_header[8..12].try_into().unwrap()) as usize;
            
            if chunk_size > 100 * 1024 * 1024 {
                return Err(FileshareError::Transfer(format!("Chunk too large: {} bytes", chunk_size)));
            }
            
            let mut chunk_data = vec![0u8; chunk_size];
            recv_stream.read_exact(&mut chunk_data).await?;
            
            state.chunk_writer.send((chunk_id, chunk_data)).await?;
            
            let received = state.received_chunks.fetch_add(1, Ordering::Relaxed) + 1;
            
            if received % 10 == 0 || received == state.total_chunks {
                let progress = (received as f64 / state.total_chunks as f64) * 100.0;
                debug!("📊 Progress: {:.1}% ({}/{})", progress, received, state.total_chunks);
            }
        }
        
        Ok(())
    }
    
    async fn process_data_stream_legacy(
        mut recv_stream: quinn::RecvStream,
        first_bytes: [u8; 4],
    ) -> Result<()> {
        // The first_bytes are the first 4 bytes of chunk_id
        let mut id_bytes = [0u8; 8];
        id_bytes[..4].copy_from_slice(&first_bytes);
        recv_stream.read_exact(&mut id_bytes[4..]).await?;
        let first_chunk_id = u64::from_be_bytes(id_bytes);
        
        // Wait for an active transfer to be available
        let start_time = Instant::now();
        let (transfer_key, state) = loop {
            if let Some(entry) = ACTIVE_TRANSFERS.iter()
                .find(|entry| !entry.value().completed.load(Ordering::Relaxed)) {
                break (entry.key().clone(), entry.value().clone());
            }
            
            if start_time.elapsed() > Duration::from_secs(5) {
                return Err(FileshareError::Transfer("No active transfer found within timeout".to_string()));
            }
            
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        
        debug!("📥 Processing legacy data stream for transfer: {}", transfer_key);
        
        // Process first chunk
        let mut size_bytes = [0u8; 4];
        recv_stream.read_exact(&mut size_bytes).await?;
        let chunk_size = u32::from_be_bytes(size_bytes) as usize;
        
        if chunk_size > 100 * 1024 * 1024 {
            return Err(FileshareError::Transfer(format!("Chunk too large: {} bytes", chunk_size)));
        }
        
        let mut chunk_data = vec![0u8; chunk_size];
        recv_stream.read_exact(&mut chunk_data).await?;
        
        state.chunk_writer.send((first_chunk_id, chunk_data)).await?;
        state.received_chunks.fetch_add(1, Ordering::Relaxed);
        
        // Process remaining chunks
        loop {
            let mut chunk_header = [0u8; 12];
            match recv_stream.read_exact(&mut chunk_header).await {
                Ok(_) => {},
                Err(_) => break,
            }
            
            let chunk_id = u64::from_be_bytes(chunk_header[0..8].try_into().unwrap());
            let chunk_size = u32::from_be_bytes(chunk_header[8..12].try_into().unwrap()) as usize;
            
            if chunk_size > 100 * 1024 * 1024 {
                return Err(FileshareError::Transfer(format!("Chunk too large: {} bytes", chunk_size)));
            }
            
            let mut chunk_data = vec![0u8; chunk_size];
            recv_stream.read_exact(&mut chunk_data).await?;
            
            state.chunk_writer.send((chunk_id, chunk_data)).await?;
            
            let received = state.received_chunks.fetch_add(1, Ordering::Relaxed) + 1;
            
            if received == state.total_chunks {
                state.completed.store(true, Ordering::Relaxed);
                info!("🎉 All chunks received for: {}", state.filename);
            }
        }
        
        Ok(())
    }
}