use crate::quic::protocol::*;
use crate::quic::stream_manager::StreamManager;
use crate::quic::progress::ProgressTracker;
use crate::Result;
use memmap2::MmapOptions;
use std::collections::HashMap;
use std::fs::File;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, RwLock, Semaphore};
use tokio::task::JoinSet;
use tracing::{info, error, warn, debug};
use uuid::Uuid;
use futures::stream::StreamExt;

/// High-performance file transfer engine using QUIC streams
/// Designed for 50-100 MBPS throughput with parallel streams
pub struct QuicFileTransfer {
    pub transfer_id: Uuid,
    pub peer_id: Uuid,
    pub metadata: QuicFileMetadata,
    pub direction: TransferDirection,
    pub status: Arc<RwLock<TransferStatus>>,
    pub progress: Arc<ProgressTracker>,
    pub stream_manager: Arc<StreamManager>,
    pub created_at: Instant,
    bytes_transferred: Arc<AtomicU64>,
    throughput_calculator: Arc<ThroughputCalculator>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TransferDirection {
    Sending,
    Receiving,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TransferStatus {
    Initializing,
    Active { streams_active: u8 },
    Paused,
    Completed,
    Failed(String),
    Cancelled,
}

pub struct QuicTransferManager {
    active_transfers: Arc<RwLock<HashMap<Uuid, Arc<QuicFileTransfer>>>>,
    stream_manager: Arc<StreamManager>,
    max_concurrent_transfers: usize,
    transfer_semaphore: Arc<Semaphore>,
}

impl QuicTransferManager {
    pub fn new(stream_manager: Arc<StreamManager>, max_concurrent: usize) -> Self {
        Self {
            active_transfers: Arc::new(RwLock::new(HashMap::new())),
            stream_manager,
            max_concurrent_transfers: max_concurrent,
            transfer_semaphore: Arc::new(Semaphore::new(max_concurrent)),
        }
    }

    /// Start a new file transfer as sender
    pub async fn send_file(
        &self,
        peer_id: Uuid,
        file_path: PathBuf,
        target_dir: Option<String>,
        stream_count: u8,
    ) -> Result<Uuid> {
        // Validate file
        if !file_path.exists() || !file_path.is_file() {
            return Err(crate::FileshareError::FileOperation("Invalid file path".to_string()));
        }

        let file_size = std::fs::metadata(&file_path)?.len();
        let transfer_id = Uuid::new_v4();

        // Create metadata
        let metadata = self.create_file_metadata(&file_path, file_size, target_dir).await?;
        
        info!("🚀 Starting QUIC file transfer: {} ({:.1} MB) to {} using {} streams",
              metadata.name, file_size as f64 / (1024.0 * 1024.0), peer_id, stream_count);

        // Create transfer object
        let transfer = Arc::new(QuicFileTransfer::new(
            transfer_id,
            peer_id,
            metadata.clone(),
            TransferDirection::Sending,
            self.stream_manager.clone(),
        ));

        // Store transfer
        {
            let mut transfers = self.active_transfers.write().await;
            transfers.insert(transfer_id, transfer.clone());
        }

        // Start the transfer
        let stream_manager = self.stream_manager.clone();
        let transfer_clone = transfer.clone();
        let file_path_clone = file_path.clone();
        let semaphore = self.transfer_semaphore.clone();

        tokio::spawn(async move {
            let _permit = semaphore.acquire().await.unwrap();
            
            if let Err(e) = Self::execute_send_transfer(
                transfer_clone,
                file_path_clone,
                stream_manager,
                stream_count,
            ).await {
                error!("❌ Transfer {} failed: {}", transfer_id, e);
                
                // Update status to failed
                let mut status = transfer.status.write().await;
                *status = TransferStatus::Failed(e.to_string());
            }
        });

        Ok(transfer_id)
    }

    async fn execute_send_transfer(
        transfer: Arc<QuicFileTransfer>,
        file_path: PathBuf,
        stream_manager: Arc<StreamManager>,
        stream_count: u8,
    ) -> Result<()> {
        // Update status to active
        {
            let mut status = transfer.status.write().await;
            *status = TransferStatus::Active { streams_active: stream_count };
        }

        // Memory map the file for zero-copy reading
        let file = File::open(&file_path)?;
        let mmap = unsafe { MmapOptions::new().map(&file)? };
        let file_data = Arc::new(mmap);
        
        info!("📁 Memory mapped file: {}", file_path.display());

        // Calculate chunk distribution across streams
        let chunk_size = transfer.metadata.chunk_size;
        let total_chunks = transfer.metadata.total_chunks;
        let chunks_per_stream = (total_chunks + stream_count as u64 - 1) / stream_count as u64;

        // Create chunk distribution map
        let mut stream_assignments = HashMap::new();
        for stream_id in 1..=stream_count {
            let start_chunk = (stream_id - 1) as u64 * chunks_per_stream;
            let end_chunk = std::cmp::min(start_chunk + chunks_per_stream, total_chunks);
            
            if start_chunk < total_chunks {
                stream_assignments.insert(stream_id, (start_chunk, end_chunk));
                debug!("📊 Stream {} assigned chunks {}-{}", stream_id, start_chunk, end_chunk - 1);
            }
        }

        // Start parallel stream senders
        let mut join_set = JoinSet::new();
        let start_time = Instant::now();

        for (stream_id, (start_chunk, end_chunk)) in stream_assignments {
            let transfer_clone = transfer.clone();
            let file_data_clone = file_data.clone();
            let stream_manager_clone = stream_manager.clone();

            join_set.spawn(async move {
                Self::send_stream_chunks(
                    transfer_clone,
                    stream_id,
                    start_chunk,
                    end_chunk,
                    chunk_size,
                    file_data_clone,
                    stream_manager_clone,
                ).await
            });
        }

        // Wait for all streams to complete
        let mut completed_streams = 0;
        let mut failed_streams = 0;

        while let Some(result) = join_set.join_next().await {
            match result {
                Ok(Ok(_)) => {
                    completed_streams += 1;
                    info!("✅ Stream completed ({}/{})", completed_streams, stream_count);
                }
                Ok(Err(e)) => {
                    failed_streams += 1;
                    error!("❌ Stream failed: {} ({} failed)", e, failed_streams);
                }
                Err(e) => {
                    failed_streams += 1;
                    error!("❌ Stream task panicked: {} ({} failed)", e, failed_streams);
                }
            }
        }

        let duration = start_time.elapsed();
        let throughput = transfer.bytes_transferred.load(Ordering::Relaxed) as f64 / duration.as_secs_f64();

        if failed_streams == 0 {
            // Update status to completed
            {
                let mut status = transfer.status.write().await;
                *status = TransferStatus::Completed;
            }

            info!("🎉 Transfer {} completed in {:.1}s - {:.1} MB/s throughput",
                  transfer.transfer_id, duration.as_secs_f64(), throughput / (1024.0 * 1024.0));
        } else {
            // Update status to failed
            {
                let mut status = transfer.status.write().await;
                *status = TransferStatus::Failed(format!("{} streams failed", failed_streams));
            }
        }

        Ok(())
    }

    async fn send_stream_chunks(
        transfer: Arc<QuicFileTransfer>,
        stream_id: u8,
        start_chunk: u64,
        end_chunk: u64,
        chunk_size: usize,
        file_data: Arc<memmap2::Mmap>,
        stream_manager: Arc<StreamManager>,
    ) -> Result<()> {
        let mut bytes_sent = 0u64;
        let stream_start = Instant::now();

        info!("📊 Stream {} starting: chunks {}-{}", stream_id, start_chunk, end_chunk - 1);

        for chunk_id in start_chunk..end_chunk {
            let chunk_start = (chunk_id * chunk_size as u64) as usize;
            let chunk_end = std::cmp::min(
                chunk_start + chunk_size,
                file_data.len()
            );
            let is_last_chunk = chunk_id == transfer.metadata.total_chunks - 1;

            // Get chunk data from memory map (zero-copy)
            let chunk_data = &file_data[chunk_start..chunk_end];
            
            // Create QUIC chunk
            let quic_chunk = QuicChunk::new(
                transfer.transfer_id,
                chunk_id,
                stream_id,
                chunk_data.to_vec(), // TODO: Optimize to avoid copy
                is_last_chunk,
            );

            // Send chunk via stream manager
            stream_manager.send_chunk(
                transfer.peer_id,
                stream_id,
                quic_chunk,
            ).await?;

            bytes_sent += chunk_data.len() as u64;
            transfer.bytes_transferred.fetch_add(chunk_data.len() as u64, Ordering::Relaxed);

            // Update progress
            transfer.progress.update_chunk_completed(chunk_id).await;

            // Log progress every 10 chunks
            if chunk_id % 10 == 0 {
                let elapsed = stream_start.elapsed().as_secs_f64();
                let stream_throughput = bytes_sent as f64 / elapsed;
                debug!("📊 Stream {} progress: chunk {}/{} - {:.1} MB/s", 
                       stream_id, chunk_id - start_chunk + 1, end_chunk - start_chunk,
                       stream_throughput / (1024.0 * 1024.0));
            }
        }

        let stream_duration = stream_start.elapsed();
        let stream_throughput = bytes_sent as f64 / stream_duration.as_secs_f64();
        
        info!("✅ Stream {} completed: {:.1} MB in {:.1}s - {:.1} MB/s",
              stream_id, bytes_sent as f64 / (1024.0 * 1024.0), 
              stream_duration.as_secs_f64(), stream_throughput / (1024.0 * 1024.0));

        Ok(())
    }

    /// Start receiving a file transfer
    pub async fn receive_file(
        &self,
        transfer_id: Uuid,
        peer_id: Uuid,
        metadata: QuicFileMetadata,
        save_path: PathBuf,
    ) -> Result<()> {
        info!("📥 Starting QUIC file receive: {} ({:.1} MB) from {}",
              metadata.name, metadata.size as f64 / (1024.0 * 1024.0), peer_id);

        // Create transfer object
        let transfer = Arc::new(QuicFileTransfer::new(
            transfer_id,
            peer_id,
            metadata,
            TransferDirection::Receiving,
            self.stream_manager.clone(),
        ));

        // Store transfer
        {
            let mut transfers = self.active_transfers.write().await;
            transfers.insert(transfer_id, transfer.clone());
        }

        // Start the receiver
        let stream_manager = self.stream_manager.clone();
        let transfer_clone = transfer.clone();
        let semaphore = self.transfer_semaphore.clone();

        tokio::spawn(async move {
            let _permit = semaphore.acquire().await.unwrap();
            
            if let Err(e) = Self::execute_receive_transfer(
                transfer_clone,
                save_path,
                stream_manager,
            ).await {
                error!("❌ Receive transfer {} failed: {}", transfer_id, e);
                
                // Update status to failed
                let mut status = transfer.status.write().await;
                *status = TransferStatus::Failed(e.to_string());
            }
        });

        Ok(())
    }

    async fn execute_receive_transfer(
        transfer: Arc<QuicFileTransfer>,
        save_path: PathBuf,
        stream_manager: Arc<StreamManager>,
    ) -> Result<()> {
        // Create file
        let file = File::create(&save_path)?;
        file.set_len(transfer.metadata.size)?;
        
        // Memory map the file for zero-copy writing
        let mmap = unsafe { MmapOptions::new().map_mut(&file)? };
        let file_data = Arc::new(std::sync::Mutex::new(mmap));
        
        info!("📁 Created and memory mapped receive file: {}", save_path.display());

        // Set status to active
        {
            let mut status = transfer.status.write().await;
            *status = TransferStatus::Active { streams_active: 8 }; // Will be updated
        }

        // Start chunk receiver
        let mut chunk_receiver = stream_manager.get_chunk_receiver(transfer.transfer_id).await?;
        let start_time = Instant::now();

        while let Some(chunk) = chunk_receiver.recv().await {
            // Write chunk to memory mapped file
            {
                let mut mmap = file_data.lock().unwrap();
                let chunk_start = (chunk.chunk_id * transfer.metadata.chunk_size as u64) as usize;
                let chunk_end = chunk_start + chunk.data.len();
                
                if chunk_end <= mmap.len() {
                    mmap[chunk_start..chunk_end].copy_from_slice(&chunk.data);
                } else {
                    error!("❌ Chunk {} out of bounds: {}-{} > {}", 
                           chunk.chunk_id, chunk_start, chunk_end, mmap.len());
                    continue;
                }
            }

            transfer.bytes_transferred.fetch_add(chunk.data.len() as u64, Ordering::Relaxed);
            transfer.progress.update_chunk_completed(chunk.chunk_id).await;

            // Check if transfer is complete
            if chunk.is_last {
                break;
            }

            // Log progress every 100 chunks
            if chunk.chunk_id % 100 == 0 {
                let elapsed = start_time.elapsed().as_secs_f64();
                let bytes_received = transfer.bytes_transferred.load(Ordering::Relaxed);
                let throughput = bytes_received as f64 / elapsed;
                
                debug!("📥 Receive progress: chunk {} - {:.1} MB/s",
                       chunk.chunk_id, throughput / (1024.0 * 1024.0));
            }
        }

        // Sync memory mapped file to disk
        {
            let mmap = file_data.lock().unwrap();
            mmap.flush()?;
        }

        let duration = start_time.elapsed();
        let total_bytes = transfer.bytes_transferred.load(Ordering::Relaxed);
        let throughput = total_bytes as f64 / duration.as_secs_f64();

        // Update status to completed
        {
            let mut status = transfer.status.write().await;
            *status = TransferStatus::Completed;
        }

        info!("🎉 Receive transfer {} completed in {:.1}s - {:.1} MB/s throughput",
              transfer.transfer_id, duration.as_secs_f64(), throughput / (1024.0 * 1024.0));

        Ok(())
    }

    async fn create_file_metadata(
        &self,
        file_path: &PathBuf,
        file_size: u64,
        target_dir: Option<String>,
    ) -> Result<QuicFileMetadata> {
        use sha2::{Digest, Sha256};

        let name = file_path
            .file_name()
            .ok_or_else(|| crate::FileshareError::FileOperation("Invalid file name".to_string()))?
            .to_string_lossy()
            .to_string();

        // Calculate optimal chunk size based on file size
        let chunk_size = Self::calculate_optimal_chunk_size(file_size);
        let total_chunks = (file_size + chunk_size as u64 - 1) / chunk_size as u64;

        // Calculate checksum (using larger buffer for speed)
        let mut hasher = Sha256::new();
        let mut file = File::open(file_path)?;
        let mut buffer = vec![0; 1024 * 1024]; // 1MB buffer
        
        loop {
            use std::io::Read;
            let bytes_read = file.read(&mut buffer)?;
            if bytes_read == 0 { break; }
            hasher.update(&buffer[..bytes_read]);
        }
        
        let checksum = format!("{:x}", hasher.finalize());

        let metadata = std::fs::metadata(file_path)?;
        let created = metadata.created().ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs());
        let modified = metadata.modified().ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs());

        Ok(QuicFileMetadata {
            name,
            size: file_size,
            checksum,
            mime_type: Self::guess_mime_type(&file_path),
            created,
            modified,
            chunk_size,
            total_chunks,
            compression: CompressionType::None, // TODO: Implement compression
            target_dir,
            resume_token: None,
        })
    }

    fn calculate_optimal_chunk_size(file_size: u64) -> usize {
        match file_size {
            0..=10_485_760 => 64 * 1024,        // <= 10MB: 64KB chunks
            10_485_761..=104_857_600 => 256 * 1024,  // 10-100MB: 256KB chunks
            104_857_601..=1_073_741_824 => 512 * 1024, // 100MB-1GB: 512KB chunks
            _ => 1024 * 1024,                   // > 1GB: 1MB chunks
        }
    }

    fn guess_mime_type(file_path: &PathBuf) -> Option<String> {
        file_path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| match ext.to_lowercase().as_str() {
                "txt" => "text/plain",
                "pdf" => "application/pdf",
                "jpg" | "jpeg" => "image/jpeg",
                "png" => "image/png",
                "mp4" => "video/mp4",
                "mp3" => "audio/mpeg",
                _ => "application/octet-stream",
            }.to_string())
    }

    /// Get transfer status
    pub async fn get_transfer_status(&self, transfer_id: Uuid) -> Option<TransferStatus> {
        let transfers = self.active_transfers.read().await;
        if let Some(transfer) = transfers.get(&transfer_id) {
            let status = transfer.status.read().await;
            Some(status.clone())
        } else {
            None
        }
    }

    /// Get transfer progress
    pub async fn get_transfer_progress(&self, transfer_id: Uuid) -> Option<(u64, u64, f64)> {
        let transfers = self.active_transfers.read().await;
        if let Some(transfer) = transfers.get(&transfer_id) {
            let bytes_transferred = transfer.bytes_transferred.load(Ordering::Relaxed);
            let throughput = transfer.throughput_calculator.get_current_throughput();
            Some((bytes_transferred, transfer.metadata.size, throughput))
        } else {
            None
        }
    }
}

impl QuicFileTransfer {
    fn new(
        transfer_id: Uuid,
        peer_id: Uuid,
        metadata: QuicFileMetadata,
        direction: TransferDirection,
        stream_manager: Arc<StreamManager>,
    ) -> Self {
        Self {
            transfer_id,
            peer_id,
            metadata: metadata.clone(),
            direction,
            status: Arc::new(RwLock::new(TransferStatus::Initializing)),
            progress: Arc::new(ProgressTracker::new(transfer_id, metadata.total_chunks)),
            stream_manager,
            created_at: Instant::now(),
            bytes_transferred: Arc::new(AtomicU64::new(0)),
            throughput_calculator: Arc::new(ThroughputCalculator::new()),
        }
    }
}

#[derive(Debug)]
pub struct ThroughputCalculator {
    samples: Arc<std::sync::Mutex<Vec<(Instant, u64)>>>,
    window_size: Duration,
}

impl ThroughputCalculator {
    pub fn new() -> Self {
        Self {
            samples: Arc::new(std::sync::Mutex::new(Vec::new())),
            window_size: Duration::from_secs(5), // 5 second window
        }
    }

    pub fn add_sample(&self, bytes: u64) {
        let mut samples = self.samples.lock().unwrap();
        let now = Instant::now();
        
        samples.push((now, bytes));
        
        // Remove old samples outside window
        samples.retain(|(timestamp, _)| now.duration_since(*timestamp) <= self.window_size);
    }

    pub fn get_current_throughput(&self) -> f64 {
        let samples = self.samples.lock().unwrap();
        if samples.len() < 2 {
            return 0.0;
        }

        let total_bytes: u64 = samples.iter().map(|(_, bytes)| bytes).sum();
        let time_span = samples.last().unwrap().0.duration_since(samples.first().unwrap().0);
        
        if time_span.as_secs_f64() > 0.0 {
            total_bytes as f64 / time_span.as_secs_f64()
        } else {
            0.0
        }
    }
}