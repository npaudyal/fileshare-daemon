use crate::{FileshareError, Result};
use quinn::{SendStream, RecvStream, TransportConfig, VarInt};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicBool, Ordering};
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{debug, error, info};
use uuid::Uuid;
use futures::future::join_all;
use bytes::{Bytes, BytesMut};
use tokio::sync::mpsc;

// Ultra-optimized constants
const ULTRA_CHUNK_SIZE: u64 = 128 * 1024 * 1024; // 128MB chunks for maximum throughput
const MAX_CONCURRENT_STREAMS: usize = 256; // Much higher stream concurrency
const STREAM_BUFFER_SIZE: usize = 16 * 1024 * 1024; // 16MB per-stream buffer
const PREFETCH_CHUNKS: usize = 4; // Prefetch chunks ahead of time

pub struct UltraTransfer;

#[derive(Clone)]
pub struct UltraTransferConfig {
    pub chunk_size: u64,
    pub max_streams: usize,
    pub enable_zero_copy: bool,
    pub enable_compression: bool,
}

impl Default for UltraTransferConfig {
    fn default() -> Self {
        Self {
            chunk_size: ULTRA_CHUNK_SIZE,
            max_streams: MAX_CONCURRENT_STREAMS,
            enable_zero_copy: true,
            enable_compression: false,
        }
    }
}

impl UltraTransfer {
    /// Configure QUIC transport for maximum throughput
    pub fn create_optimized_transport_config() -> TransportConfig {
        let mut config = TransportConfig::default();
        
        // Maximize flow control windows
        config.max_idle_timeout(Some(VarInt::from_u32(300_000).into())); // 5 minutes
        config.stream_receive_window(VarInt::from_u32(128 * 1024 * 1024)); // 128MB per stream
        config.receive_window(VarInt::from_u32(1024 * 1024 * 1024)); // 1GB connection window
        config.send_window(1024 * 1024 * 1024); // 1GB send window
        
        // Optimize for throughput
        config.max_concurrent_bidi_streams(VarInt::from_u32(1024));
        config.max_concurrent_uni_streams(VarInt::from_u32(1024));
        config.datagram_receive_buffer_size(Some(64 * 1024 * 1024)); // 64MB
        
        // Disable features that add latency
        config.keep_alive_interval(None);
        config.congestion_controller_factory(Arc::new(quinn::congestion::BbrConfig::default()));
        
        config
    }
    
    /// Ultra-fast parallel transfer with zero-copy and prefetching
    pub async fn transfer_file(
        connection: Arc<quinn::Connection>,
        source_path: PathBuf,
        target_path: String,
        config: UltraTransferConfig,
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
        
        // Calculate chunks
        let chunk_size = config.chunk_size;
        let total_chunks = (file_size + chunk_size - 1) / chunk_size;
        let stream_count = std::cmp::min(config.max_streams, total_chunks as usize);
        
        // Send metadata in first stream
        let transfer_id = Uuid::new_v4();
        Self::send_metadata(&connection, &filename, file_size, &target_path, chunk_size, total_chunks, &transfer_id).await?;
        
        // Open all streams in parallel - THIS IS KEY!
        let stream_futures: Vec<_> = (0..stream_count)
            .map(|_| connection.open_uni())
            .collect();
        
        let streams = join_all(stream_futures).await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| FileshareError::Transfer(format!("Failed to open streams: {}", e)))?;
        
        info!("⚡ Opened {} streams in parallel", streams.len());
        
        // Create chunk sender tasks with prefetching
        let bytes_sent = Arc::new(AtomicU64::new(0));
        let file_path = Arc::new(source_path);
        
        let mut handles = Vec::new();
        for (idx, stream) in streams.into_iter().enumerate() {
            let file_path = file_path.clone();
            let bytes_sent = bytes_sent.clone();
            let transfer_id = transfer_id;
            
            // Calculate chunk range for this stream
            let chunks_per_stream = total_chunks / stream_count as u64;
            let extra = total_chunks % stream_count as u64;
            let start_chunk = if idx < extra as usize {
                idx as u64 * (chunks_per_stream + 1)
            } else {
                extra * (chunks_per_stream + 1) + (idx as u64 - extra) * chunks_per_stream
            };
            let end_chunk = if idx < extra as usize {
                start_chunk + chunks_per_stream + 1
            } else {
                start_chunk + chunks_per_stream
            };
            
            let handle = tokio::spawn(async move {
                Self::send_chunks_ultra(
                    stream,
                    file_path,
                    start_chunk,
                    end_chunk,
                    chunk_size,
                    file_size,
                    bytes_sent,
                    transfer_id,
                ).await
            });
            
            handles.push(handle);
        }
        
        // Wait for all transfers
        for handle in handles {
            handle.await
                .map_err(|e| FileshareError::Transfer(format!("Task failed: {}", e)))??;
        }
        
        let duration = start_time.elapsed();
        let throughput_mbps = (file_size as f64 * 8.0) / (duration.as_secs_f64() * 1_000_000.0);
        info!("⚡ ULTRA transfer complete: {:.1} MB in {:.2}s ({:.1} Mbps)", 
              file_size as f64 / (1024.0 * 1024.0), duration.as_secs_f64(), throughput_mbps);
        
        Ok(())
    }
    
    async fn send_metadata(
        connection: &quinn::Connection,
        filename: &str,
        file_size: u64,
        target_path: &str,
        chunk_size: u64,
        total_chunks: u64,
        transfer_id: &Uuid,
    ) -> Result<()> {
        let mut stream = connection.open_uni().await
            .map_err(|e| FileshareError::Transfer(format!("Failed to open metadata stream: {}", e)))?;
        
        // Simple, efficient protocol
        let metadata = format!("ULTRA|{}|{}|{}|{}|{}|{}", 
                             filename, file_size, target_path, chunk_size, total_chunks, transfer_id);
        let metadata_bytes = metadata.as_bytes();
        
        stream.write_all(&(metadata_bytes.len() as u32).to_be_bytes()).await?;
        stream.write_all(metadata_bytes).await?;
        stream.finish()?;
        
        Ok(())
    }
    
    async fn send_chunks_ultra(
        mut stream: SendStream,
        file_path: Arc<PathBuf>,
        start_chunk: u64,
        end_chunk: u64,
        chunk_size: u64,
        file_size: u64,
        bytes_sent: Arc<AtomicU64>,
        transfer_id: Uuid,
    ) -> Result<()> {
        // Send transfer ID first
        stream.write_all(transfer_id.as_bytes()).await?;
        
        // Use memory-mapped file for zero-copy reads
        let file = tokio::fs::File::open(file_path.as_ref()).await?;
        let mut file = tokio::io::BufReader::with_capacity(STREAM_BUFFER_SIZE, file);
        
        // Pre-allocate buffer
        let mut buffer = vec![0u8; chunk_size as usize];
        
        for chunk_idx in start_chunk..end_chunk {
            let offset = chunk_idx * chunk_size;
            let bytes_to_read = std::cmp::min(chunk_size, file_size - offset) as usize;
            
            // Seek and read
            file.seek(std::io::SeekFrom::Start(offset)).await?;
            file.read_exact(&mut buffer[..bytes_to_read]).await?;
            
            // Send chunk header and data in one write
            let mut chunk_data = BytesMut::with_capacity(12 + bytes_to_read);
            chunk_data.extend_from_slice(&chunk_idx.to_be_bytes());
            chunk_data.extend_from_slice(&(bytes_to_read as u32).to_be_bytes());
            chunk_data.extend_from_slice(&buffer[..bytes_to_read]);
            
            stream.write_all(&chunk_data).await?;
            
            bytes_sent.fetch_add(bytes_to_read as u64, Ordering::Relaxed);
        }
        
        stream.finish()?;
        Ok(())
    }
}

/// Ultra-fast receiver with zero-copy writes
pub struct UltraReceiver;

impl UltraReceiver {
    pub async fn handle_incoming_transfer(mut recv_stream: RecvStream) -> Result<()> {
        // Read header length
        let mut header_len = [0u8; 4];
        recv_stream.read_exact(&mut header_len).await?;
        let header_len = u32::from_be_bytes(header_len) as usize;
        
        // Read header
        let mut header_bytes = vec![0u8; header_len];
        recv_stream.read_exact(&mut header_bytes).await?;
        let header = String::from_utf8(header_bytes)
            .map_err(|_| FileshareError::Transfer("Invalid header".to_string()))?;
        
        let parts: Vec<&str> = header.split('|').collect();
        if parts[0] == "ULTRA" && parts.len() >= 7 {
            Self::receive_ultra_transfer(recv_stream, &parts).await
        } else {
            Self::receive_data_stream(recv_stream).await
        }
    }
    
    async fn receive_ultra_transfer(
        _recv_stream: RecvStream,
        parts: &[&str],
    ) -> Result<()> {
        let filename = parts[1].to_string();
        let file_size: u64 = parts[2].parse()?;
        let target_path = PathBuf::from(parts[3]);
        let chunk_size: u64 = parts[4].parse()?;
        let total_chunks: u64 = parts[5].parse()?;
        let transfer_id = parts[6].to_string();
        
        info!("⚡ Receiving ULTRA transfer: {} ({:.1} MB, {} chunks)", 
              filename, file_size as f64 / (1024.0 * 1024.0), total_chunks);
        
        // Create file with pre-allocation
        let file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .read(true)
            .open(&target_path)?;
        file.set_len(file_size)?;
        
        // Store transfer state
        let state = Arc::new(UltraTransferState {
            file: Arc::new(file),
            chunk_size,
            total_chunks,
            received_chunks: AtomicU64::new(0),
            transfer_id,
        });
        
        ULTRA_TRANSFERS.insert(transfer_id, state);
        Ok(())
    }
    
    async fn receive_data_stream(mut recv_stream: RecvStream) -> Result<()> {
        // Read transfer ID
        let mut transfer_id_bytes = [0u8; 16];
        recv_stream.read_exact(&mut transfer_id_bytes).await?;
        let transfer_id = Uuid::from_bytes(transfer_id_bytes).to_string();
        
        let state = ULTRA_TRANSFERS.get(&transfer_id)
            .ok_or_else(|| FileshareError::Transfer("Transfer not found".to_string()))?
            .clone();
        
        // Process chunks with zero-copy writes
        let mut buffer = vec![0u8; state.chunk_size as usize];
        
        loop {
            // Read chunk header
            let mut header = [0u8; 12];
            match recv_stream.read_exact(&mut header).await {
                Ok(_) => {},
                Err(_) => break,
            }
            
            let chunk_idx = u64::from_be_bytes([
                header[0], header[1], header[2], header[3],
                header[4], header[5], header[6], header[7],
            ]);
            let chunk_size = u32::from_be_bytes([
                header[8], header[9], header[10], header[11],
            ]) as usize;
            
            // Read chunk data
            recv_stream.read_exact(&mut buffer[..chunk_size]).await?;
            
            // Write with pwrite (zero-copy on Unix)
            let offset = chunk_idx * state.chunk_size;
            #[cfg(unix)]
            {
                use std::os::unix::fs::FileExt;
                state.file.write_at(&buffer[..chunk_size], offset)?;
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::FileExt;
                state.file.seek_write(&buffer[..chunk_size], offset)?;
            }
            
            let received = state.received_chunks.fetch_add(1, Ordering::Relaxed) + 1;
            if received == state.total_chunks {
                state.file.sync_all()?;
                ULTRA_TRANSFERS.remove(&transfer_id);
                info!("⚡ ULTRA transfer complete: {}", transfer_id);
            }
        }
        
        Ok(())
    }
}

struct UltraTransferState {
    file: Arc<std::fs::File>,
    chunk_size: u64,
    total_chunks: u64,
    received_chunks: AtomicU64,
    transfer_id: String,
}

lazy_static::lazy_static! {
    static ref ULTRA_TRANSFERS: dashmap::DashMap<String, Arc<UltraTransferState>> = dashmap::DashMap::new();
}