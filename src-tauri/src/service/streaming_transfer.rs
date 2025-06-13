use crate::network::{streaming_protocol::*, streaming_reader::*, streaming_writer::*};
use crate::{FileshareError, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncWriteExt, AsyncReadExt};
use tokio::net::{TcpStream, TcpListener};
use tokio::sync::{mpsc, RwLock, Mutex};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct TransferStats {
    pub total_bytes: u64,
    pub chunk_count: u64,
    pub duration: Duration,
    pub average_speed_mbps: f64,
    pub compression_ratio: f64,
}

#[derive(Debug, Clone)]
pub struct TransferProgress {
    pub transfer_id: Uuid,
    pub bytes_transferred: u64,
    pub total_bytes: u64,
    pub chunks_completed: u64,
    pub total_chunks: u64,
    pub current_speed_mbps: f64,
    pub eta_seconds: f64,
    pub started_at: Instant,
    pub last_update: Instant,
}

impl TransferProgress {
    pub fn new(transfer_id: Uuid, total_bytes: u64, total_chunks: u64) -> Self {
        let now = Instant::now();
        Self {
            transfer_id,
            bytes_transferred: 0,
            total_bytes,
            chunks_completed: 0,
            total_chunks,
            current_speed_mbps: 0.0,
            eta_seconds: 0.0,
            started_at: now,
            last_update: now,
        }
    }
    
    pub fn update(&mut self, bytes_transferred: u64, chunks_completed: u64) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.started_at).as_secs_f64();
        
        self.bytes_transferred = bytes_transferred;
        self.chunks_completed = chunks_completed;
        self.last_update = now;
        
        if elapsed > 0.0 {
            self.current_speed_mbps = (bytes_transferred as f64 / (1024.0 * 1024.0)) / elapsed;
            
            let remaining_bytes = self.total_bytes - bytes_transferred;
            if self.current_speed_mbps > 0.0 {
                self.eta_seconds = (remaining_bytes as f64 / (1024.0 * 1024.0)) / self.current_speed_mbps;
            }
        }
    }
    
    pub fn progress_percentage(&self) -> f64 {
        if self.total_bytes == 0 {
            return 100.0;
        }
        (self.bytes_transferred as f64 / self.total_bytes as f64) * 100.0
    }
}

pub struct StreamingTransferManager {
    config: StreamingConfig,
    active_outgoing: Arc<RwLock<HashMap<Uuid, Arc<Mutex<TransferProgress>>>>>,
    active_incoming: Arc<RwLock<HashMap<Uuid, Arc<Mutex<TransferProgress>>>>>,
    transfer_stats: Arc<RwLock<HashMap<Uuid, TransferStats>>>,
    connections: Arc<RwLock<HashMap<Uuid, TcpStream>>>,
}

impl StreamingTransferManager {
    pub fn new(config: StreamingConfig) -> Self {
        info!("🚀 Creating StreamingTransferManager with config: chunk_size={}KB, compression={}",
              config.base_chunk_size / 1024, config.enable_compression);
        
        Self {
            config,
            active_outgoing: Arc::new(RwLock::new(HashMap::new())),
            active_incoming: Arc::new(RwLock::new(HashMap::new())),
            transfer_stats: Arc::new(RwLock::new(HashMap::new())),
            connections: Arc::new(RwLock::new(HashMap::new())),
        }
    }
    
    // Start streaming file to peer
    pub async fn start_streaming_transfer(
        &self,
        peer_id: Uuid,
        file_path: PathBuf,
        stream: TcpStream,
    ) -> Result<Uuid> {
        let transfer_id = Uuid::new_v4();
        
        // Validate file exists and get size
        let file_metadata = std::fs::metadata(&file_path)
            .map_err(|e| FileshareError::FileOperation(format!("Cannot access file: {}", e)))?;
        
        let file_size = file_metadata.len();
        let file_name = file_path.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        info!(
            "🚀 STREAMING: Starting transfer {} to peer {}: {} ({:.1} MB)",
            transfer_id, peer_id, file_name, file_size as f64 / (1024.0 * 1024.0)
        );

        // Create metadata for the transfer
        let metadata = self.create_metadata(&file_path).await?;
        
        // Create progress tracker
        let progress = Arc::new(Mutex::new(TransferProgress::new(
            transfer_id,
            file_size,
            metadata.estimated_chunks,
        )));
        
        {
            let mut outgoing = self.active_outgoing.write().await;
            outgoing.insert(transfer_id, progress.clone());
        }

        // Show initial notification
        notify_rust::Notification::new()
            .summary("🚀 High-Speed Transfer Starting")
            .body(&format!(
                "📁 {}\n📊 {:.1} MB\n🔄 Initializing streaming protocol...",
                file_name, file_size as f64 / (1024.0 * 1024.0)
            ))
            .timeout(notify_rust::Timeout::Milliseconds(3000))
            .show()
            .map_err(|e| FileshareError::Unknown(format!("Notification error: {}", e)))?;

        // Start streaming in background task
        let config = self.config;
        let stats_store = self.transfer_stats.clone();
        let outgoing_store = self.active_outgoing.clone();
        
        tokio::spawn(async move {
            let start_time = Instant::now();
            
            match Self::execute_streaming_transfer(
                transfer_id,
                file_path.clone(),
                stream,
                config,
                progress,
            ).await {
                Ok(stats) => {
                    let duration = start_time.elapsed();
                    let final_stats = TransferStats {
                        total_bytes: stats.total_bytes,
                        chunk_count: stats.chunk_count,
                        duration,
                        average_speed_mbps: (stats.total_bytes as f64 / (1024.0 * 1024.0)) / duration.as_secs_f64(),
                        compression_ratio: stats.compression_ratio,
                    };
                    
                    // Store final stats
                    {
                        let mut stats_map = stats_store.write().await;
                        stats_map.insert(transfer_id, final_stats.clone());
                    }
                    
                    info!(
                        "✅ STREAMING: Transfer {} completed successfully in {:.1}s ({:.1} MB/s, {:.1}% compression)",
                        transfer_id, 
                        duration.as_secs_f64(), 
                        final_stats.average_speed_mbps,
                        (1.0 - final_stats.compression_ratio) * 100.0
                    );
                    
                    // Show completion notification with detailed stats
                    let _ = notify_rust::Notification::new()
                        .summary("✅ High-Speed Transfer Complete")
                        .body(&format!(
                            "📁 {}\n📊 {:.1} MB in {:.1}s\n⚡ {:.1} MB/s average\n🗜️ {:.1}% compression savings",
                            file_name,
                            final_stats.total_bytes as f64 / (1024.0 * 1024.0),
                            duration.as_secs_f64(),
                            final_stats.average_speed_mbps,
                            (1.0 - final_stats.compression_ratio) * 100.0
                        ))
                        .timeout(notify_rust::Timeout::Milliseconds(5000))
                        .show();
                }
                Err(e) => {
                    error!("❌ STREAMING: Transfer {} failed: {}", transfer_id, e);
                    
                    let _ = notify_rust::Notification::new()
                        .summary("❌ High-Speed Transfer Failed")
                        .body(&format!(
                            "📁 {}\n❌ Error: {}\n🔄 You can try again",
                            file_name, e
                        ))
                        .timeout(notify_rust::Timeout::Milliseconds(5000))
                        .show();
                }
            }
            
            // Clean up progress tracker
            {
                let mut outgoing = outgoing_store.write().await;
                outgoing.remove(&transfer_id);
            }
        });

        Ok(transfer_id)
    }
    
    // Execute the actual streaming transfer
    async fn execute_streaming_transfer(
        transfer_id: Uuid,
        file_path: PathBuf,
        stream: TcpStream,
        config: StreamingConfig,
        progress: Arc<Mutex<TransferProgress>>,
    ) -> Result<StreamingStats> {
        info!("🚀 EXECUTE: Starting streaming execution for transfer {}", transfer_id);
        
        // Create streaming reader
        let reader = StreamingFileReader::new(file_path, transfer_id, config).await?;
        let mut chunk_stream = reader.create_chunk_stream().await?;
        
        // Execute the transfer with progress tracking
        Self::stream_file_chunks_with_progress(chunk_stream, stream, config, progress).await
    }
    
    // Stream file chunks with detailed progress tracking
    async fn stream_file_chunks_with_progress(
        mut chunk_stream: StreamingChunkReader,
        mut stream: TcpStream,
        _config: StreamingConfig,
        progress: Arc<Mutex<TransferProgress>>,
    ) -> Result<StreamingStats> {
        let mut total_bytes = 0u64;
        let mut total_uncompressed_bytes = 0u64;
        let mut chunk_count = 0u64;
        let start_time = Instant::now();
        let mut last_progress_update = Instant::now();
        
        info!("📤 STREAMING: Starting chunk transmission");
        
        while let Some((header, data)) = chunk_stream.next_chunk().await? {
            // Send header first
            let header_bytes = header.to_bytes();
            stream.write_all(&header_bytes).await
                .map_err(|e| {
                    error!("❌ Failed to send header for chunk {}: {}", header.get_chunk_index(), e);
                    FileshareError::Network(e)
                })?;
            
            // Send chunk data
            stream.write_all(&data).await
                .map_err(|e| {
                    error!("❌ Failed to send data for chunk {}: {}", header.get_chunk_index(), e);
                    FileshareError::Network(e)
                })?;
            
            // Update statistics
            let data_size = data.len() as u64;
            total_bytes += data_size;
            chunk_count += 1;
            
            // Track compression ratio
            if header.is_compressed() {
                // Estimate original size (this is approximate)
                total_uncompressed_bytes += (data_size as f64 * 1.3) as u64; // Assume ~30% compression
            } else {
                total_uncompressed_bytes += data_size;
            }
            
            // Update progress every 50 chunks or every 2 seconds
            let now = Instant::now();
            if chunk_count % 50 == 0 || now.duration_since(last_progress_update).as_secs() >= 2 {
                {
                    let mut prog = progress.lock().await;
                    prog.update(total_bytes, chunk_count);
                }
                
                let elapsed = start_time.elapsed().as_secs_f64();
                let speed_mbps = if elapsed > 0.0 {
                    (total_bytes as f64 / (1024.0 * 1024.0)) / elapsed
                } else {
                    0.0
                };
                
                debug!(
                    "📤 STREAMING: Chunk {} sent - {:.1} MB total @ {:.1} MB/s",
                    chunk_count, total_bytes as f64 / (1024.0 * 1024.0), speed_mbps
                );
                
                last_progress_update = now;
            }
            
            if header.is_last_chunk() {
                info!("📤 STREAMING: Last chunk {} sent, finalizing transfer", chunk_count);
                break;
            }
            
            // Small delay to prevent overwhelming the receiver (adaptive)
            if chunk_count % 100 == 0 {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        }
        
        // Flush the stream
        stream.flush().await
            .map_err(|e| {
                error!("❌ Failed to flush stream: {}", e);
                FileshareError::Network(e)
            })?;
        
        // Graceful shutdown
        stream.shutdown().await
            .map_err(|e| {
                warn!("⚠️ Warning during stream shutdown: {}", e);
                // Don't fail the transfer for shutdown issues
                e
            }).unwrap_or(());
        
        let compression_ratio = if total_uncompressed_bytes > 0 {
            total_bytes as f64 / total_uncompressed_bytes as f64
        } else {
            1.0
        };
        
        info!(
            "✅ STREAMING: Transfer completed - {} chunks, {:.1} MB, {:.1}% compression",
            chunk_count,
            total_bytes as f64 / (1024.0 * 1024.0),
            (1.0 - compression_ratio) * 100.0
        );
        
        Ok(StreamingStats {
            total_bytes,
            chunk_count,
            compression_ratio,
        })
    }
    
    // Receive streaming file from peer
    // FIXED: Clone the file_name before the async block to avoid move issues
pub async fn receive_streaming_transfer(
    &self,
    transfer_id: Uuid,
    metadata: StreamingFileMetadata,
    save_path: PathBuf,
    stream: TcpStream,
) -> Result<()> {
    info!(
        "📥 STREAMING: Starting receive for transfer {}: {} ({:.1} MB)",
        transfer_id, metadata.name, metadata.size as f64 / (1024.0 * 1024.0)
    );
    
    // Create progress tracker
    let progress = Arc::new(Mutex::new(TransferProgress::new(
        transfer_id,
        metadata.size,
        metadata.estimated_chunks,
    )));
    
    {
        let mut incoming = self.active_incoming.write().await;
        incoming.insert(transfer_id, progress.clone());
    }
    
    // Show receive notification
    notify_rust::Notification::new()
        .summary("📥 Receiving High-Speed Transfer")
        .body(&format!(
            "📁 {}\n📊 {:.1} MB\n🔄 Preparing to receive...",
            metadata.name, metadata.size as f64 / (1024.0 * 1024.0)
        ))
        .timeout(notify_rust::Timeout::Milliseconds(3000))
        .show()
        .map_err(|e| FileshareError::Unknown(format!("Notification error: {}", e)))?;

    // FIXED: Clone values that will be moved into the async block
    let config = self.config;
    let stats_store = self.transfer_stats.clone();
    let incoming_store = self.active_incoming.clone();
    let file_name = metadata.name.clone(); // Clone before moving metadata
    let save_path_clone = save_path.clone(); // Clone for notification
    
    tokio::spawn(async move {
        let start_time = Instant::now();
        
        match Self::execute_streaming_receive(
            transfer_id,
            metadata, // metadata moved here
            save_path,
            stream,
            config,
            progress,
        ).await {
            Ok(stats) => {
                let duration = start_time.elapsed();
                let final_stats = TransferStats {
                    total_bytes: stats.total_bytes,
                    chunk_count: stats.chunk_count,
                    duration,
                    average_speed_mbps: (stats.total_bytes as f64 / (1024.0 * 1024.0)) / duration.as_secs_f64(),
                    compression_ratio: stats.compression_ratio,
                };
                
                // Store final stats
                {
                    let mut stats_map = stats_store.write().await;
                    stats_map.insert(transfer_id, final_stats.clone());
                }
                
                info!(
                    "✅ STREAMING: Receive {} completed in {:.1}s ({:.1} MB/s)",
                    transfer_id, duration.as_secs_f64(), final_stats.average_speed_mbps
                );
                
                // FIXED: Use cloned file_name and save_path_clone
                let _ = notify_rust::Notification::new()
                    .summary("✅ High-Speed Transfer Received")
                    .body(&format!(
                        "📁 {}\n📊 {:.1} MB received in {:.1}s\n⚡ {:.1} MB/s average\n📂 Saved to: {}",
                        file_name, // Now uses cloned value
                        final_stats.total_bytes as f64 / (1024.0 * 1024.0),
                        duration.as_secs_f64(),
                        final_stats.average_speed_mbps,
                        save_path_clone.display() // Uses cloned value
                    ))
                    .timeout(notify_rust::Timeout::Milliseconds(5000))
                    .show();
            }
            Err(e) => {
                error!("❌ STREAMING: Receive {} failed: {}", transfer_id, e);
                
                // FIXED: Use cloned file_name
                let _ = notify_rust::Notification::new()
                    .summary("❌ High-Speed Transfer Failed")
                    .body(&format!(
                        "📁 {}\n❌ Receive failed: {}",
                        file_name, e // Now uses cloned value - no more borrow error!
                    ))
                    .timeout(notify_rust::Timeout::Milliseconds(5000))
                    .show();
            }
        }
        
        // Clean up progress tracker
        {
            let mut incoming = incoming_store.write().await;
            incoming.remove(&transfer_id);
        }
    });

    Ok(())
}
    
    // Execute streaming receive
    async fn execute_streaming_receive(
        transfer_id: Uuid,
        metadata: StreamingFileMetadata,
        save_path: PathBuf,
        stream: TcpStream,
        config: StreamingConfig,
        progress: Arc<Mutex<TransferProgress>>,
    ) -> Result<StreamingStats> {
        info!("📥 EXECUTE: Starting streaming receive for transfer {}", transfer_id);
        
        // Create streaming writer
        let mut writer = StreamingFileWriter::new(
            save_path,
            transfer_id,
            config,
            metadata.size,
            metadata.estimated_chunks,
        ).await?;
        
        // Receive chunks with progress tracking
        Self::receive_file_chunks_with_progress(writer, stream, progress).await
    }
    
    // Receive file chunks with progress tracking
    async fn receive_file_chunks_with_progress(
        mut writer: StreamingFileWriter,
        mut stream: TcpStream,
        progress: Arc<Mutex<TransferProgress>>,
    ) -> Result<StreamingStats> {
        let mut header_buffer = [0u8; StreamChunkHeader::SIZE];
        let mut total_bytes = 0u64;
        let mut chunk_count = 0u64;
        let mut last_progress_update = Instant::now();
        
        info!("📥 STREAMING: Starting chunk reception");
        
        loop {
            // Read header
            stream.read_exact(&mut header_buffer).await
                .map_err(|e| {
                    error!("❌ Failed to read chunk header: {}", e);
                    FileshareError::Network(e)
                })?;
            
            let header = StreamChunkHeader::from_bytes(&header_buffer);
            let chunk_index = header.get_chunk_index();
            let chunk_size = header.get_chunk_size();
            
            // Read chunk data
            let mut chunk_data = vec![0u8; chunk_size as usize];
            stream.read_exact(&mut chunk_data).await
                .map_err(|e| {
                    error!("❌ Failed to read chunk {} data: {}", chunk_index, e);
                    FileshareError::Network(e)
                })?;
            
            let data = bytes::Bytes::from(chunk_data);
            
            // Write chunk to file
            let is_complete = writer.write_chunk(header, data.clone()).await
                .map_err(|e| {
                    error!("❌ Failed to write chunk {}: {}", chunk_index, e);
                    e
                })?;
            
            // Update statistics
            total_bytes += data.len() as u64;
            chunk_count += 1;
            
            // Update progress every 50 chunks or every 2 seconds
            let now = Instant::now();
            if chunk_count % 50 == 0 || now.duration_since(last_progress_update).as_secs() >= 2 {
                {
                    let mut prog = progress.lock().await;
                    prog.update(total_bytes, chunk_count);
                }
                
                debug!(
                    "📥 STREAMING: Received chunk {} - {:.1} MB total",
                    chunk_count, total_bytes as f64 / (1024.0 * 1024.0)
                );
                
                last_progress_update = now;
            }
            
            if is_complete || header.is_last_chunk() {
                info!("📥 STREAMING: Receive completed after {} chunks", chunk_count);
                break;
            }
        }
        
        Ok(StreamingStats {
            total_bytes,
            chunk_count,
            compression_ratio: 1.0, // Calculate from actual data if needed
        })
    }
    
    // Create metadata for streaming transfer
    // FIXED: Create metadata for streaming transfer
async fn create_metadata(&self, file_path: &PathBuf) -> Result<StreamingFileMetadata> {
    use sha2::{Digest, Sha256};
    
    let file = std::fs::File::open(file_path)
        .map_err(|e| FileshareError::FileOperation(format!("Cannot open file: {}", e)))?;
    
    let metadata = file.metadata()
        .map_err(|e| FileshareError::FileOperation(format!("Cannot get metadata: {}", e)))?;
    
    let file_size = metadata.len();
    let chunk_size = self.config.base_chunk_size;
    let estimated_chunks = (file_size + chunk_size as u64 - 1) / chunk_size as u64;
    
    // Calculate file hash in background (optional for performance)
    let file_hash = "streaming_placeholder".to_string(); // TODO: Calculate if needed
    
    let name = file_path.file_name()
        .ok_or_else(|| FileshareError::FileOperation("Invalid filename".to_string()))?
        .to_string_lossy()
        .to_string();
    
    // FIXED: Calculate mime_type BEFORE moving name into the struct
    let mime_type = Self::guess_mime_type(&name);
    
    Ok(StreamingFileMetadata {
        name, // Now moved here after mime_type calculation
        size: file_size,
        modified: metadata.modified().ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs()),
        mime_type, // Use the pre-calculated value
        target_dir: None,
        suggested_chunk_size: chunk_size,
        supports_compression: self.config.enable_compression,
        estimated_chunks,
        file_hash,
    })
}
    
    // Simple MIME type detection
    fn guess_mime_type(filename: &str) -> Option<String> {
        let extension = std::path::Path::new(filename)
            .extension()?
            .to_str()?
            .to_lowercase();
        
        match extension.as_str() {
            "mp4" | "mov" | "avi" | "mkv" => Some("video/mp4".to_string()),
            "mp3" | "wav" | "flac" => Some("audio/mpeg".to_string()),
            "jpg" | "jpeg" | "png" | "gif" => Some("image/jpeg".to_string()),
            "pdf" => Some("application/pdf".to_string()),
            "txt" => Some("text/plain".to_string()),
            "zip" | "rar" | "7z" => Some("application/zip".to_string()),
            _ => None,
        }
    }
    
    // Get progress for outgoing transfer
    pub async fn get_outgoing_progress(&self, transfer_id: Uuid) -> Option<TransferProgress> {
        let outgoing = self.active_outgoing.read().await;
        if let Some(progress_arc) = outgoing.get(&transfer_id) {
            let progress = progress_arc.lock().await;
            Some(progress.clone())
        } else {
            None
        }
    }
    
    // Get progress for incoming transfer  
    pub async fn get_incoming_progress(&self, transfer_id: Uuid) -> Option<TransferProgress> {
        let incoming = self.active_incoming.read().await;
        if let Some(progress_arc) = incoming.get(&transfer_id) {
            let progress = progress_arc.lock().await;
            Some(progress.clone())
        } else {
            None
        }
    }
    
    // Get transfer statistics
    pub async fn get_transfer_stats(&self, transfer_id: Uuid) -> Option<TransferStats> {
        let stats = self.transfer_stats.read().await;
        stats.get(&transfer_id).cloned()
    }
    
    // Get all active transfers
    pub async fn get_active_transfers(&self) -> Vec<Uuid> {
        let mut transfers = Vec::new();
        
        {
            let outgoing = self.active_outgoing.read().await;
            transfers.extend(outgoing.keys().cloned());
        }
        
        {
            let incoming = self.active_incoming.read().await;
            transfers.extend(incoming.keys().cloned());
        }
        
        transfers
    }
    
    // Cancel a transfer
    pub async fn cancel_transfer(&self, transfer_id: Uuid) -> Result<()> {
        info!("🛑 Cancelling streaming transfer {}", transfer_id);
        
        // Remove from active transfers
        {
            let mut outgoing = self.active_outgoing.write().await;
            outgoing.remove(&transfer_id);
        }
        
        {
            let mut incoming = self.active_incoming.write().await;
            incoming.remove(&transfer_id);
        }
        
        // Close connection if exists
        {
            let mut connections = self.connections.write().await;
            if let Some(mut stream) = connections.remove(&transfer_id) {
                let _ = stream.shutdown().await;
            }
        }
        
        info!("✅ Transfer {} cancelled", transfer_id);
        Ok(())
    }
    
    // Cleanup completed transfers older than 1 hour
    pub async fn cleanup_old_transfers(&self) {
        let cutoff = Instant::now() - Duration::from_secs(3600); // 1 hour
        
        let mut stats = self.transfer_stats.write().await;
        let initial_count = stats.len();
        
        stats.retain(|_, stat| {
            stat.duration < cutoff.elapsed()
        });
        
        let removed = initial_count - stats.len();
        if removed > 0 {
            info!("🧹 Cleaned up {} old transfer statistics", removed);
        }
    }
}

#[derive(Debug, Clone)]
struct StreamingStats {
    total_bytes: u64,
    chunk_count: u64,
    compression_ratio: f64,
}

// Background cleanup task
impl StreamingTransferManager {
    pub async fn start_cleanup_task(&self) {
        let stats_store = self.transfer_stats.clone();
        
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1800)); // 30 minutes
            
            loop {
                interval.tick().await;
                
                let cutoff = Instant::now() - Duration::from_secs(3600); // 1 hour
                
                let mut stats = stats_store.write().await;
                let initial_count = stats.len();
                
                stats.retain(|_, stat| {
                    stat.duration < cutoff.elapsed()
                });
                
                let removed = initial_count - stats.len();
                if removed > 0 {
                    info!("🧹 Background cleanup removed {} old transfer records", removed);
                }
            }
        });
    }
}