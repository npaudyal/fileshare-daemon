use crate::quic::stream_manager::StreamManager;
use crate::{FileshareError, Result};
use dashmap::DashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicBool, AtomicUsize, Ordering};
use std::time::{Instant, Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt, AsyncSeekExt};
use tokio::sync::{Semaphore, RwLock, mpsc};
use tracing::{debug, error, info};
use uuid::Uuid;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

// Optimized constants for maximum performance
const SMALL_FILE_THRESHOLD: u64 = 10 * 1024 * 1024; // 10MB
const OPTIMAL_CHUNK_SIZE: u64 = 16 * 1024 * 1024; // 16MB default
const MAX_PARALLEL_STREAMS: usize = 64; // Maximum concurrent streams
const INITIAL_STREAMS: usize = 4; // Start with fewer streams
const STREAM_RAMP_UP_INTERVAL: Duration = Duration::from_millis(500);
const WRITE_CONCURRENCY: usize = 32; // Concurrent disk writes
const PROBE_SIZE: usize = 10 * 1024 * 1024; // 10MB bandwidth probe

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
        
        // Probe bandwidth first for LAN transfers
        let bandwidth_mbps = Self::probe_bandwidth(&stream_manager).await?;
        info!("🔍 Detected bandwidth: {:.1} Mbps", bandwidth_mbps);
        
        let result = if file_size <= SMALL_FILE_THRESHOLD {
            Self::single_stream_transfer(stream_manager, source_path, target_path, filename, file_size).await
        } else {
            Self::blazing_parallel_transfer(
                stream_manager, source_path, target_path, filename, file_size, bandwidth_mbps
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
    
    async fn probe_bandwidth(stream_manager: &Arc<StreamManager>) -> Result<f64> {
        let mut stream = stream_manager.open_file_transfer_streams(1).await?
            .into_iter().next()
            .ok_or_else(|| FileshareError::Transfer("Failed to create probe stream".to_string()))?;
        
        let probe_data = vec![0u8; PROBE_SIZE];
        let start = Instant::now();
        
        stream.write_all(b"PROBE|").await?;
        stream.write_all(&probe_data).await?;
        stream.finish()?;
        
        let elapsed = start.elapsed();
        let bandwidth_mbps = (PROBE_SIZE as f64 * 8.0) / (elapsed.as_secs_f64() * 1_000_000.0);
        
        Ok(bandwidth_mbps)
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
        bandwidth_mbps: f64,
    ) -> Result<()> {
        let chunk_size = Self::calculate_optimal_chunk_size(file_size, bandwidth_mbps);
        let total_chunks = (file_size + chunk_size - 1) / chunk_size;
        let optimal_streams = Self::calculate_optimal_streams(file_size, bandwidth_mbps, total_chunks);
        
        info!("🚀 BLAZING parallel transfer: up to {} streams, {} chunks of {:.1} MB each", 
              optimal_streams, total_chunks, chunk_size as f64 / (1024.0 * 1024.0));
        
        // Send control message
        let control_stream = stream_manager.open_file_transfer_streams(1).await?
            .into_iter().next()
            .ok_or_else(|| FileshareError::Transfer("Failed to create control stream".to_string()))?;
            
        Self::send_control_message(
            control_stream, &filename, file_size, &target_path, chunk_size, total_chunks
        ).await?;
        
        // Create progress tracking
        let bytes_sent = Arc::new(AtomicU64::new(0));
        let active_streams = Arc::new(AtomicUsize::new(0));
        let chunks_sent = Arc::new(AtomicU64::new(0));
        let (chunk_tx, _) = tokio::sync::broadcast::channel(total_chunks as usize);
        
        // Fill chunk queue
        for chunk_idx in 0..total_chunks {
            chunk_tx.send(chunk_idx).map_err(|e| FileshareError::Transfer(format!("Chunk send error: {e}")))?;
        }
        
        // Keep chunk_tx alive for creating receivers
        let chunk_tx = Arc::new(chunk_tx);
        
        // Progressive stream launcher
        let stream_manager_clone = stream_manager.clone();
        let file_path = Arc::new(source_path);
        let bytes_sent_clone = bytes_sent.clone();
        let active_streams_clone = active_streams.clone();
        let chunks_sent_clone = chunks_sent.clone();
        let chunk_tx_clone = chunk_tx.clone();
        
        let stream_launcher = tokio::spawn(async move {
            Self::progressive_stream_launcher(
                stream_manager_clone,
                file_path,
                chunk_tx_clone,
                chunk_size,
                file_size,
                total_chunks,
                optimal_streams,
                bytes_sent_clone,
                active_streams_clone,
                chunks_sent_clone,
            ).await
        });
        
        // Progress monitor
        let monitor_handle = tokio::spawn(Self::monitor_progress(
            bytes_sent.clone(),
            active_streams.clone(),
            chunks_sent.clone(),
            file_size,
            total_chunks,
            Instant::now(),
        ));
        
        // Wait for completion
        stream_launcher.await??;
        monitor_handle.abort();
        
        Ok(())
    }
    
    async fn progressive_stream_launcher(
        stream_manager: Arc<StreamManager>,
        file_path: Arc<PathBuf>,
        chunk_tx: Arc<tokio::sync::broadcast::Sender<u64>>,
        chunk_size: u64,
        file_size: u64,
        total_chunks: u64,
        max_streams: usize,
        bytes_sent: Arc<AtomicU64>,
        active_streams: Arc<AtomicUsize>,
        chunks_sent: Arc<AtomicU64>,
    ) -> Result<()> {
        let mut handles = Vec::new();
        let mut interval = tokio::time::interval(STREAM_RAMP_UP_INTERVAL);
        
        // Start with initial streams
        for stream_idx in 0..INITIAL_STREAMS.min(max_streams) {
            if let Ok(stream) = stream_manager.open_file_transfer_streams(1).await?
                .into_iter().next()
                .ok_or_else(|| FileshareError::Transfer("Failed to create stream".to_string())) 
            {
                active_streams.fetch_add(1, Ordering::Relaxed);
                
                let file_path = file_path.clone();
                let bytes_sent = bytes_sent.clone();
                let active_streams = active_streams.clone();
                let chunks_sent = chunks_sent.clone();
                let mut chunk_rx = chunk_tx.subscribe();
                
                let handle = tokio::spawn(async move {
                    let result = Self::stream_chunk_sender(
                        stream,
                        stream_idx,
                        file_path,
                        &mut chunk_rx,
                        chunk_size,
                        file_size,
                        bytes_sent,
                        chunks_sent,
                    ).await;
                    
                    active_streams.fetch_sub(1, Ordering::Relaxed);
                    result
                });
                
                handles.push(handle);
            }
        }
        
        // Progressively add more streams based on performance
        let mut current_streams = INITIAL_STREAMS;
        while current_streams < max_streams && chunks_sent.load(Ordering::Relaxed) < total_chunks {
            interval.tick().await;
            
            let progress = chunks_sent.load(Ordering::Relaxed) as f64 / total_chunks as f64;
            let should_add_stream = progress > 0.1 && current_streams < max_streams;
            
            if should_add_stream {
                if let Ok(stream) = stream_manager.open_file_transfer_streams(1).await?
                    .into_iter().next()
                    .ok_or_else(|| FileshareError::Transfer("Failed to create stream".to_string())) 
                {
                    current_streams += 1;
                    active_streams.fetch_add(1, Ordering::Relaxed);
                    
                    let file_path = file_path.clone();
                    let bytes_sent = bytes_sent.clone();
                    let active_streams_clone = active_streams.clone();
                    let chunks_sent = chunks_sent.clone();
                    let mut chunk_rx = chunk_tx.subscribe();
                    
                    let handle = tokio::spawn(async move {
                        let result = Self::stream_chunk_sender(
                            stream,
                            current_streams - 1,
                            file_path,
                            &mut chunk_rx,
                            chunk_size,
                            file_size,
                            bytes_sent,
                            chunks_sent,
                        ).await;
                        
                        active_streams_clone.fetch_sub(1, Ordering::Relaxed);
                        result
                    });
                    
                    handles.push(handle);
                    info!("📈 Added stream {} (total: {})", current_streams, current_streams);
                }
            }
        }
        
        // Wait for all streams to complete
        for handle in handles {
            handle.await??;
        }
        
        Ok(())
    }
    
    async fn stream_chunk_sender(
        mut stream: quinn::SendStream,
        stream_idx: usize,
        file_path: Arc<PathBuf>,
        chunk_rx: &mut tokio::sync::broadcast::Receiver<u64>,
        chunk_size: u64,
        file_size: u64,
        bytes_sent: Arc<AtomicU64>,
        chunks_sent: Arc<AtomicU64>,
    ) -> Result<()> {
        let mut file = tokio::fs::File::open(file_path.as_ref()).await?;
        let mut buffer = vec![0u8; chunk_size as usize];
        let mut sent_count = 0;
        
        loop {
            match chunk_rx.recv().await {
                Ok(chunk_idx) => {
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
                    sent_count += 1;
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    // We missed some messages, but that's OK - other streams will handle them
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    // All chunks have been sent
                    break;
                }
            }
        }
        
        stream.finish()?;
        debug!("✅ Stream {} completed: sent {} chunks", stream_idx, sent_count);
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
        active_streams: Arc<AtomicUsize>,
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
            let current_streams = active_streams.load(Ordering::Relaxed);
            
            let interval_bytes = current_bytes - last_bytes;
            let interval_time = last_time.elapsed();
            let interval_speed_mbps = (interval_bytes as f64 * 8.0) / 
                (interval_time.as_secs_f64() * 1_000_000.0);
            
            let overall_speed_mbps = (current_bytes as f64 * 8.0) / 
                (start_time.elapsed().as_secs_f64() * 1_000_000.0);
            
            let progress = (current_chunks as f64 / total_chunks as f64) * 100.0;
            
            if current_bytes < file_size {
                info!("📊 Progress: {:.1}% - Speed: {:.1} Mbps (avg: {:.1} Mbps) - Streams: {} - Chunks: {}/{}", 
                      progress, interval_speed_mbps, overall_speed_mbps, current_streams, 
                      current_chunks, total_chunks);
            }
            
            last_bytes = current_bytes;
            last_time = Instant::now();
            
            if current_bytes >= file_size {
                break;
            }
        }
    }
    
    fn calculate_optimal_chunk_size(file_size: u64, bandwidth_mbps: f64) -> u64 {
        let base_chunk = match file_size {
            0..=100_000_000 => 8 * 1024 * 1024,
            100_000_001..=500_000_000 => 16 * 1024 * 1024,
            500_000_001..=2_000_000_000 => 32 * 1024 * 1024,
            _ => 64 * 1024 * 1024,
        };
        
        // Adjust based on bandwidth
        if bandwidth_mbps > 1000.0 {
            base_chunk * 2
        } else {
            base_chunk
        }
    }
    
    fn calculate_optimal_streams(file_size: u64, bandwidth_mbps: f64, total_chunks: u64) -> usize {
        let base_streams = match file_size {
            0..=100_000_000 => 8,
            100_000_001..=500_000_000 => 16,
            500_000_001..=2_000_000_000 => 32,
            _ => 64,
        };
        
        let bandwidth_factor = (bandwidth_mbps / 100.0).max(1.0);
        let optimal = (base_streams as f64 * bandwidth_factor) as usize;
        
        optimal.min(MAX_PARALLEL_STREAMS).min(total_chunks as usize)
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
        let header_len = u32::from_be_bytes(len_bytes) as usize;
        
        if header_len < 10 || header_len > 1000 {
            return Self::process_data_stream(recv_stream, len_bytes).await;
        }
        
        let mut header_bytes = vec![0u8; header_len];
        recv_stream.read_exact(&mut header_bytes).await?;
        
        // Check for probe
        if &header_bytes[..6] == b"PROBE|" {
            // Just consume the probe data
            let mut buffer = vec![0u8; 8192];
            while recv_stream.read(&mut buffer).await?.is_some() {}
            return Ok(());
        }
        
        let header = String::from_utf8(header_bytes)?;
        let parts: Vec<&str> = header.split('|').collect();
        
        match parts[0] {
            "BLAZING_SINGLE" => Self::receive_single_stream(recv_stream, &parts).await,
            "BLAZING_PARALLEL" => Self::receive_parallel_control(recv_stream, &parts).await,
            _ => Err(FileshareError::Transfer("Invalid header".to_string())),
        }
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
        
        // Create file with async fallocate
        let file = tokio::fs::File::create(&target_path).await?;
        
        #[cfg(target_os = "macos")]
{
    // macOS doesn't have fallocate, just set the file length
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
    // Windows or other platforms: just set length
    file.set_len(file_size).await?;
}
        
        // Create chunk writer channel
        let (chunk_tx, mut chunk_rx) = mpsc::channel::<(u64, Vec<u8>)>(100);
        
        // Spawn writer task for sequential writes
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
        
        let state = Arc::new(TransferState {
            filename: filename.clone(),
            file_size,
            chunk_size,
            total_chunks,
            target_path,
            received_chunks: AtomicU64::new(0),
            write_semaphore: Arc::new(Semaphore::new(WRITE_CONCURRENCY)),
            completed: AtomicBool::new(false),
            chunk_writer: chunk_tx,
        });
        
        ACTIVE_TRANSFERS.insert(filename.clone(), state);
        
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
    
    async fn process_data_stream(
        mut recv_stream: quinn::RecvStream,
        first_bytes: [u8; 4],
    ) -> Result<()> {
        let mut id_bytes = [0u8; 8];
        id_bytes[..4].copy_from_slice(&first_bytes);
        recv_stream.read_exact(&mut id_bytes[4..]).await?;
        let first_chunk_id = u64::from_be_bytes(id_bytes);
        
        let transfer_key = ACTIVE_TRANSFERS.iter()
            .find(|entry| !entry.value().completed.load(Ordering::Relaxed))
            .map(|entry| entry.key().clone())
            .ok_or_else(|| FileshareError::Transfer("No active transfer".to_string()))?;
        
        let state = ACTIVE_TRANSFERS.get(&transfer_key)
            .ok_or_else(|| FileshareError::Transfer("Transfer not found".to_string()))?
            .clone();
        
        // Process first chunk
        let mut size_bytes = [0u8; 4];
        recv_stream.read_exact(&mut size_bytes).await?;
        let chunk_size = u32::from_be_bytes(size_bytes) as usize;
        
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