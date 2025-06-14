use crate::network::streaming_protocol::*;
use crate::{FileshareError, Result};
use bytes::Bytes;
use lz4_flex::decompress_size_prepended;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::fs::File;
use tokio::io::{AsyncSeekExt, AsyncWriteExt, BufWriter, SeekFrom};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

// ENHANCED: Progress tracking with detailed metrics
#[derive(Debug, Clone)]
pub enum ProgressUpdate {
    Started {
        transfer_id: Uuid,
        expected_size: u64,
        expected_chunks: u64,
    },
    Progress {
        transfer_id: Uuid,
        bytes_written: u64,
        chunks_written: u64,
        write_speed_mbps: f64,
        eta_seconds: f64,
    },
    Completed {
        transfer_id: Uuid,
        final_size: u64,
        total_chunks: u64,
        duration: Duration,
        average_speed_mbps: f64,
    },
    Failed {
        transfer_id: Uuid,
        error: String,
        bytes_written: u64,
    },
    Cancelled {
        transfer_id: Uuid,
        bytes_written: u64,
    },
}

// ENHANCED: Chunk data with metadata for debugging
#[derive(Debug, Clone)]
struct ChunkData {
    data: Vec<u8>,
    received_at: Instant,
    chunk_index: u64,
    is_compressed: bool,
    original_size: Option<usize>, // For compressed chunks
}

impl ChunkData {
    fn new(data: Vec<u8>, chunk_index: u64, is_compressed: bool) -> Self {
        Self {
            data,
            received_at: Instant::now(),
            chunk_index,
            is_compressed,
            original_size: None,
        }
    }

    fn age(&self) -> Duration {
        self.received_at.elapsed()
    }

    fn size(&self) -> usize {
        self.data.len()
    }
}

// ENHANCED: Write statistics for monitoring
#[derive(Debug, Clone)]
pub struct WriteStatistics {
    pub bytes_written: u64,
    pub chunks_written: u64,
    pub out_of_order_chunks: usize,
    pub buffer_memory_usage: usize,
    pub sequential_writes: u64,
    pub seek_writes: u64,
    pub compression_saves: u64, // Bytes saved through compression
    pub write_speed_mbps: f64,
    pub efficiency_percentage: f64, // Sequential vs out-of-order ratio
}

// ENHANCED: High-performance sequential writer with advanced buffering
pub struct StreamingFileWriter {
    // Core file handling
    file: BufWriter<File>,
    file_path: PathBuf,
    transfer_id: Uuid,
    config: StreamingConfig,

    // Size and progress tracking
    expected_size: u64,
    bytes_written: u64,
    expected_chunks: u64,
    chunks_written: u64,

    // Sequential writing optimization
    next_expected_chunk: u64,
    out_of_order_chunks: BTreeMap<u64, ChunkData>,
    max_out_of_order_buffer: usize,

    // Performance optimization
    write_buffer: Vec<u8>,
    buffer_threshold: usize,
    sequential_write_count: u64,
    seek_write_count: u64,

    // State management
    completed: bool,
    cancelled: bool,
    started_at: Instant,
    last_progress_report: Instant,

    // Progress reporting
    progress_tx: Option<mpsc::UnboundedSender<ProgressUpdate>>,
    progress_report_interval: Duration,

    // Configuration
    cleanup_on_drop: bool,
    partial_file_cleanup: bool,
    sync_frequency: u64, // Sync every N chunks

    // Performance metrics
    total_compression_saves: u64,
    last_write_time: Instant,
    write_times: Vec<Duration>, // For speed calculation
}

impl StreamingFileWriter {
    pub async fn new(
        file_path: PathBuf,
        transfer_id: Uuid,
        config: StreamingConfig,
        expected_size: u64,
        expected_chunks: u64,
    ) -> Result<Self> {
        Self::new_with_options(
            file_path,
            transfer_id,
            config,
            expected_size,
            expected_chunks,
            WriterOptions::default(),
        )
        .await
    }

    pub async fn new_with_options(
        file_path: PathBuf,
        transfer_id: Uuid,
        config: StreamingConfig,
        expected_size: u64,
        expected_chunks: u64,
        options: WriterOptions,
    ) -> Result<Self> {
        // Validate inputs
        if expected_size == 0 {
            return Err(FileshareError::FileOperation(
                "Expected size cannot be zero".to_string(),
            ));
        }
        if expected_chunks == 0 {
            return Err(FileshareError::FileOperation(
                "Expected chunks cannot be zero".to_string(),
            ));
        }

        // Create parent directory if needed
        if let Some(parent) = file_path.parent() {
            if !parent.exists() {
                tokio::fs::create_dir_all(parent).await.map_err(|e| {
                    FileshareError::FileOperation(format!("Cannot create directory: {}", e))
                })?;
            }
        }

        // Create file with optimized settings
        let file = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&file_path)
            .await
            .map_err(|e| FileshareError::FileOperation(format!("Cannot create file: {}", e)))?;

        // Pre-allocate file space for better performance (try, but don't fail if not supported)
        if let Err(e) = file.set_len(expected_size).await {
            warn!(
                "Could not pre-allocate file space: {} (continuing anyway)",
                e
            );
        }

        // Calculate optimal buffer sizes
        let write_buffer_size = std::cmp::min(
            config.base_chunk_size * 8, // 8 chunks worth
            2 * 1024 * 1024,            // Maximum 2MB write buffer
        );

        let file_buffer_size = std::cmp::min(
            config.base_chunk_size * 4, // 4 chunks worth
            1024 * 1024,                // Maximum 1MB file buffer
        );

        let buffered_file = BufWriter::with_capacity(file_buffer_size, file);

        // Calculate out-of-order buffer limit
        let max_out_of_order_buffer = std::cmp::min(
            config.memory_limit / 4, // Use 25% of memory limit
            100 * 1024 * 1024,       // Maximum 100MB
        );

        let now = Instant::now();

        info!(
            "📝 StreamingFileWriter created: {} ({:.1}MB expected, {} chunks)",
            file_path.display(),
            expected_size as f64 / (1024.0 * 1024.0),
            expected_chunks
        );
        info!(
            "   Write buffer: {}KB, File buffer: {}KB, OOO buffer: {}MB",
            write_buffer_size / 1024,
            file_buffer_size / 1024,
            max_out_of_order_buffer / (1024 * 1024)
        );

        Ok(Self {
            file: buffered_file,
            file_path,
            transfer_id,
            config,
            expected_size,
            bytes_written: 0,
            expected_chunks,
            chunks_written: 0,
            next_expected_chunk: 0,
            out_of_order_chunks: BTreeMap::new(),
            max_out_of_order_buffer,
            write_buffer: Vec::with_capacity(write_buffer_size),
            buffer_threshold: write_buffer_size / 2,
            sequential_write_count: 0,
            seek_write_count: 0,
            completed: false,
            cancelled: false,
            started_at: now,
            last_progress_report: now,
            progress_tx: None,
            progress_report_interval: options.progress_report_interval,
            cleanup_on_drop: options.cleanup_on_drop,
            partial_file_cleanup: options.partial_file_cleanup,
            sync_frequency: options.sync_frequency,
            total_compression_saves: 0,
            last_write_time: now,
            write_times: Vec::with_capacity(100), // Keep last 100 write times for speed calc
        })
    }

    // ENHANCED: Write chunk with comprehensive error handling and metrics
    pub async fn write_chunk(&mut self, header: StreamChunkHeader, data: Bytes) -> Result<bool> {
        if self.completed {
            warn!(
                "⚠️ Received chunk for already completed transfer {}",
                self.transfer_id
            );
            return Ok(true);
        }

        if self.cancelled {
            return Err(FileshareError::Transfer(
                "Transfer was cancelled".to_string(),
            ));
        }

        let write_start = Instant::now();

        // Validate chunk
        let transfer_id = header.get_transfer_id();
        let chunk_index = header.get_chunk_index();
        let chunk_size = header.get_chunk_size();

        if transfer_id != self.transfer_id {
            return Err(FileshareError::Transfer(
                "Chunk belongs to different transfer".to_string(),
            ));
        }

        if chunk_size != data.len() as u32 {
            return Err(FileshareError::Transfer(format!(
                "Chunk size mismatch: header says {}, got {}",
                chunk_size,
                data.len()
            )));
        }

        // Verify checksum
        if !header.verify_checksum(&data) {
            return Err(FileshareError::Transfer(format!(
                "Checksum verification failed for chunk {}",
                chunk_index
            )));
        }

        // Decompress if needed
        let (chunk_data, decompression_savings) = if header.is_compressed() {
            let decompressed = tokio::task::spawn_blocking({
                let data = data.to_vec();
                move || decompress_size_prepended(&data)
            })
            .await
            .map_err(|e| FileshareError::Unknown(format!("Decompression task failed: {}", e)))?
            .map_err(|e| FileshareError::Transfer(format!("Decompression failed: {}", e)))?;

            let savings = decompressed.len().saturating_sub(data.len());
            (decompressed, savings as u64)
        } else {
            (data.to_vec(), 0)
        };

        self.total_compression_saves += decompression_savings;

        // Create chunk data structure
        let chunk_data_struct = ChunkData::new(chunk_data, chunk_index, header.is_compressed());

        // Handle sequential vs out-of-order chunks
        let write_result = if chunk_index == self.next_expected_chunk {
            // Sequential chunk - FAST PATH
            self.write_sequential_chunk(chunk_data_struct).await?;

            // Process any buffered sequential chunks
            while let Some(buffered_chunk) =
                self.out_of_order_chunks.remove(&self.next_expected_chunk)
            {
                self.write_sequential_chunk(buffered_chunk).await?;
            }

            true
        } else if chunk_index > self.next_expected_chunk {
            // Out-of-order chunk - buffer it
            self.buffer_out_of_order_chunk(chunk_data_struct).await?;
            false
        } else {
            // Duplicate or old chunk - ignore
            debug!("📦 Ignoring duplicate/old chunk {}", chunk_index);
            false
        };

        // Record write timing for speed calculation
        let write_duration = write_start.elapsed();
        self.write_times.push(write_duration);
        if self.write_times.len() > 100 {
            self.write_times.remove(0); // Keep only last 100 measurements
        }
        self.last_write_time = Instant::now();

        // Send progress update if needed
        self.maybe_send_progress_update().await;

        // Check completion
        let is_complete = header.is_last_chunk() || self.is_transfer_complete();
        if is_complete && !self.completed {
            self.finalize_transfer().await?;
        }

        Ok(is_complete)
    }

    // FAST: Sequential write without seeking
    async fn write_sequential_chunk(&mut self, chunk_data: ChunkData) -> Result<()> {
        let chunk_size = chunk_data.size();

        // Write directly to buffered file (sequential, no seek needed!)
        self.file.write_all(&chunk_data.data).await.map_err(|e| {
            FileshareError::FileOperation(format!("Sequential write failed: {}", e))
        })?;

        self.bytes_written += chunk_size as u64;
        self.chunks_written += 1;
        self.next_expected_chunk += 1;
        self.sequential_write_count += 1;

        debug!(
            "✅ Sequential write chunk {} ({} bytes) - {}/{} chunks, {:.1}% complete",
            self.next_expected_chunk - 1,
            chunk_size,
            self.chunks_written,
            self.expected_chunks,
            (self.chunks_written as f64 / self.expected_chunks as f64) * 100.0
        );

        // Periodic sync for durability vs performance balance
        if self.chunks_written % self.sync_frequency == 0 {
            self.file.flush().await.map_err(|e| {
                FileshareError::FileOperation(format!("Periodic flush failed: {}", e))
            })?;
        }

        Ok(())
    }

    // Buffer out-of-order chunks with intelligent memory management
    async fn buffer_out_of_order_chunk(&mut self, chunk_data: ChunkData) -> Result<()> {
        let chunk_index = chunk_data.chunk_index;
        let chunk_size = chunk_data.size();

        // Check buffer limits
        let current_buffer_size: usize = self
            .out_of_order_chunks
            .values()
            .map(|chunk| chunk.size())
            .sum();

        if current_buffer_size + chunk_size > self.max_out_of_order_buffer {
            // Try to make space by writing some out-of-order chunks
            let freed = self.emergency_flush_out_of_order_chunks().await?;

            info!(
                "🔄 Emergency flush freed {}KB, attempting to buffer chunk {}",
                freed / 1024,
                chunk_index
            );

            // Check again after emergency flush
            let current_buffer_size: usize = self
                .out_of_order_chunks
                .values()
                .map(|chunk| chunk.size())
                .sum();

            if current_buffer_size + chunk_size > self.max_out_of_order_buffer {
                return Err(FileshareError::Transfer(format!(
                    "Out-of-order buffer overflow: current={}KB, needed={}KB, max={}KB",
                    current_buffer_size / 1024,
                    chunk_size / 1024,
                    self.max_out_of_order_buffer / 1024
                )));
            }
        }

        // Check for stale chunks and warn
        let stale_threshold = Duration::from_secs(30);
        if let Some(oldest_chunk) = self
            .out_of_order_chunks
            .values()
            .min_by_key(|c| c.received_at)
        {
            if oldest_chunk.age() > stale_threshold {
                warn!(
                    "⚠️ Detected stale out-of-order chunk {} (age: {:.1}s)",
                    oldest_chunk.chunk_index,
                    oldest_chunk.age().as_secs_f64()
                );
            }
        }

        self.out_of_order_chunks.insert(chunk_index, chunk_data);

        debug!(
            "📦 Buffered out-of-order chunk {} ({} chunks buffered, {:.1}KB total)",
            chunk_index,
            self.out_of_order_chunks.len(),
            current_buffer_size as f64 / 1024.0
        );

        Ok(())
    }

    // Emergency flush of out-of-order chunks using seeking (slower but necessary)
    async fn emergency_flush_out_of_order_chunks(&mut self) -> Result<usize> {
        if self.out_of_order_chunks.is_empty() {
            return Ok(0);
        }

        warn!(
            "⚠️ Emergency flush: writing {} out-of-order chunks using seeks",
            self.out_of_order_chunks.len()
        );

        // Flush current buffer first to ensure consistency
        self.file
            .flush()
            .await
            .map_err(|e| FileshareError::FileOperation(format!("Pre-seek flush failed: {}", e)))?;

        // Calculate which chunks we can write (must be within reasonable range)
        let current_position = self.bytes_written;
        let max_seek_distance = self.config.base_chunk_size as u64 * 1000; // Don't seek too far

        let mut chunks_to_write = Vec::new();
        let mut freed_bytes = 0;

        for (&chunk_index, chunk_data) in &self.out_of_order_chunks {
            let expected_offset = chunk_index * self.config.base_chunk_size as u64;

            // Only flush chunks that are within reasonable seek distance
            if expected_offset < current_position + max_seek_distance {
                chunks_to_write.push((chunk_index, chunk_data.clone()));
                freed_bytes += chunk_data.size();
            }
        }

        // Sort chunks by index for more efficient seeking
        chunks_to_write.sort_by_key(|(index, _)| *index);

        let mut successful_writes = 0;
        for (chunk_index, chunk_data) in chunks_to_write {
            // Calculate file offset
            let file_offset = chunk_index * self.config.base_chunk_size as u64;

            // Seek and write
            match self.file.seek(SeekFrom::Start(file_offset)).await {
                Ok(_) => {
                    match self.file.write_all(&chunk_data.data).await {
                        Ok(_) => {
                            self.bytes_written += chunk_data.size() as u64;
                            self.chunks_written += 1;
                            self.seek_write_count += 1;
                            successful_writes += 1;

                            // Remove from buffer
                            self.out_of_order_chunks.remove(&chunk_index);

                            debug!(
                                "📝 Emergency wrote chunk {} at offset {}",
                                chunk_index, file_offset
                            );
                        }
                        Err(e) => {
                            error!("Failed to write out-of-order chunk {}: {}", chunk_index, e);
                            break; // Stop on first write failure
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to seek for chunk {}: {}", chunk_index, e);
                    break; // Stop on first seek failure
                }
            }
        }

        // Reset to end of file for sequential writes
        if let Err(e) = self.file.seek(SeekFrom::End(0)).await {
            error!("Failed to seek to end after emergency flush: {}", e);
        }

        // Final flush
        self.file
            .flush()
            .await
            .map_err(|e| FileshareError::FileOperation(format!("Post-seek flush failed: {}", e)))?;

        info!(
            "✅ Emergency flush completed: {} chunks written out-of-order, {}KB freed",
            successful_writes,
            freed_bytes / 1024
        );

        Ok(freed_bytes)
    }

    // Check if transfer is complete
    fn is_transfer_complete(&self) -> bool {
        self.chunks_written >= self.expected_chunks
            || (self.bytes_written >= self.expected_size && self.out_of_order_chunks.is_empty())
    }

    // Send progress update if enough time has passed
    async fn maybe_send_progress_update(&mut self) {
        if self.last_progress_report.elapsed() < self.progress_report_interval {
            return;
        }

        if let Some(ref sender) = self.progress_tx {
            let write_speed = self.calculate_current_write_speed();
            let eta = self.calculate_eta(write_speed);

            let _ = sender.send(ProgressUpdate::Progress {
                transfer_id: self.transfer_id,
                bytes_written: self.bytes_written,
                chunks_written: self.chunks_written,
                write_speed_mbps: write_speed,
                eta_seconds: eta,
            });
        }

        self.last_progress_report = Instant::now();
    }

    // Calculate current write speed based on recent measurements
    fn calculate_current_write_speed(&self) -> f64 {
        if self.write_times.is_empty() || self.bytes_written == 0 {
            return 0.0;
        }

        let total_time: Duration = self.write_times.iter().sum();
        if total_time.is_zero() {
            return 0.0;
        }

        let recent_chunk_size = self.config.base_chunk_size as f64;
        let chunks_per_second = self.write_times.len() as f64 / total_time.as_secs_f64();
        let bytes_per_second = chunks_per_second * recent_chunk_size;

        bytes_per_second / (1024.0 * 1024.0) // Convert to MB/s
    }

    // Calculate estimated time to completion
    fn calculate_eta(&self, write_speed_mbps: f64) -> f64 {
        if write_speed_mbps <= 0.0 {
            return f64::INFINITY;
        }

        let remaining_bytes = self.expected_size.saturating_sub(self.bytes_written);
        let remaining_mb = remaining_bytes as f64 / (1024.0 * 1024.0);

        remaining_mb / write_speed_mbps
    }

    // Finalize transfer with comprehensive validation
    async fn finalize_transfer(&mut self) -> Result<()> {
        if self.completed {
            return Ok(());
        }

        info!("🎯 Finalizing transfer {}", self.transfer_id);

        // Write any remaining out-of-order chunks
        if !self.out_of_order_chunks.is_empty() {
            warn!(
                "Writing {} remaining out-of-order chunks during finalization",
                self.out_of_order_chunks.len()
            );
            self.emergency_flush_out_of_order_chunks().await?;
        }

        // Final flush and sync
        self.file
            .flush()
            .await
            .map_err(|e| FileshareError::FileOperation(format!("Final flush failed: {}", e)))?;

        // Get the inner file for sync operations
        let inner_file = self.file.get_mut();
        inner_file
            .sync_all()
            .await
            .map_err(|e| FileshareError::FileOperation(format!("Final sync failed: {}", e)))?;

        // Adjust file size if needed
        if self.bytes_written != self.expected_size {
            info!(
                "Adjusting file size: expected {} bytes, wrote {} bytes",
                self.expected_size, self.bytes_written
            );

            inner_file.set_len(self.bytes_written).await.map_err(|e| {
                FileshareError::FileOperation(format!("Size adjustment failed: {}", e))
            })?;
        }

        self.completed = true;
        let duration = self.started_at.elapsed();
        let avg_speed = if duration.as_secs_f64() > 0.0 {
            (self.bytes_written as f64 / (1024.0 * 1024.0)) / duration.as_secs_f64()
        } else {
            0.0
        };

        // Send completion notification
        if let Some(ref sender) = self.progress_tx {
            let _ = sender.send(ProgressUpdate::Completed {
                transfer_id: self.transfer_id,
                final_size: self.bytes_written,
                total_chunks: self.chunks_written,
                duration,
                average_speed_mbps: avg_speed,
            });
        }

        info!("🎉 Transfer {} completed successfully:", self.transfer_id);
        info!(
            "   📁 File: {} ({:.1}MB)",
            self.file_path.display(),
            self.bytes_written as f64 / (1024.0 * 1024.0)
        );
        info!(
            "   📊 Stats: {} chunks, {:.1}% efficiency, {:.1}MB/s average",
            self.chunks_written,
            (self.sequential_write_count as f64 / self.chunks_written as f64) * 100.0,
            avg_speed
        );
        info!(
            "   💾 Compression saved: {:.1}KB",
            self.total_compression_saves as f64 / 1024.0
        );

        Ok(())
    }

    // ENHANCED: Graceful cancellation with proper cleanup
    pub async fn cancel(&mut self) -> Result<()> {
        if self.completed || self.cancelled {
            return Ok(());
        }

        warn!("🛑 Cancelling transfer {}", self.transfer_id);
        self.cancelled = true;

        // Flush any pending data
        if let Err(e) = self.file.flush().await {
            error!("Failed to flush during cancellation: {}", e);
        }

        // Send cancellation notification
        if let Some(ref sender) = self.progress_tx {
            let _ = sender.send(ProgressUpdate::Cancelled {
                transfer_id: self.transfer_id,
                bytes_written: self.bytes_written,
            });
        }

        // Clean up partial file if configured
        if self.partial_file_cleanup && self.file_path.exists() {
            if let Err(e) = tokio::fs::remove_file(&self.file_path).await {
                error!("Failed to remove partial file during cancellation: {}", e);
            } else {
                info!("🗑️ Removed partial file: {:?}", self.file_path);
            }
        }

        Ok(())
    }

    // PUBLIC API METHODS
    pub fn set_progress_sender(&mut self, tx: mpsc::UnboundedSender<ProgressUpdate>) {
        self.progress_tx = Some(tx);

        // Send initial progress
        if let Some(ref sender) = self.progress_tx {
            let _ = sender.send(ProgressUpdate::Started {
                transfer_id: self.transfer_id,
                expected_size: self.expected_size,
                expected_chunks: self.expected_chunks,
            });
        }
    }

    pub fn set_cleanup_on_drop(&mut self, cleanup: bool) {
        self.cleanup_on_drop = cleanup;
    }

    pub fn set_partial_file_cleanup(&mut self, cleanup: bool) {
        self.partial_file_cleanup = cleanup;
    }

    pub fn progress(&self) -> f32 {
        if self.expected_chunks == 0 {
            return 0.0;
        }
        (self.chunks_written as f32 / self.expected_chunks as f32).min(1.0)
    }

    pub fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    pub fn is_completed(&self) -> bool {
        self.completed
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled
    }

    pub fn file_path(&self) -> &PathBuf {
        &self.file_path
    }

    pub fn get_statistics(&self) -> WriteStatistics {
        let efficiency = if self.chunks_written > 0 {
            (self.sequential_write_count as f64 / self.chunks_written as f64) * 100.0
        } else {
            100.0
        };

        WriteStatistics {
            bytes_written: self.bytes_written,
            chunks_written: self.chunks_written,
            out_of_order_chunks: self.out_of_order_chunks.len(),
            buffer_memory_usage: self.out_of_order_chunks.values().map(|c| c.size()).sum(),
            sequential_writes: self.sequential_write_count,
            seek_writes: self.seek_write_count,
            compression_saves: self.total_compression_saves,
            write_speed_mbps: self.calculate_current_write_speed(),
            efficiency_percentage: efficiency,
        }
    }

    pub fn get_detailed_status(&self) -> DetailedWriterStatus {
        DetailedWriterStatus {
            transfer_id: self.transfer_id,
            file_path: self.file_path.clone(),
            bytes_written: self.bytes_written,
            expected_size: self.expected_size,
            chunks_written: self.chunks_written,
            expected_chunks: self.expected_chunks,
            out_of_order_count: self.out_of_order_chunks.len(),
            buffer_usage: self.out_of_order_chunks.values().map(|c| c.size()).sum(),
            completed: self.completed,
            cancelled: self.cancelled,
            progress_percentage: self.progress(),
            duration: self.started_at.elapsed(),
            statistics: self.get_statistics(),
        }
    }
}

// ENHANCED: Configuration options for writer behavior
#[derive(Debug, Clone)]
pub struct WriterOptions {
    pub progress_report_interval: Duration,
    pub cleanup_on_drop: bool,
    pub partial_file_cleanup: bool,
    pub sync_frequency: u64, // Sync every N chunks
}

impl Default for WriterOptions {
    fn default() -> Self {
        Self {
            progress_report_interval: Duration::from_secs(1),
            cleanup_on_drop: true,
            partial_file_cleanup: true,
            sync_frequency: 50, // Sync every 50 chunks
        }
    }
}

// ENHANCED: Detailed status structure
#[derive(Debug, Clone)]
pub struct DetailedWriterStatus {
    pub transfer_id: Uuid,
    pub file_path: PathBuf,
    pub bytes_written: u64,
    pub expected_size: u64,
    pub chunks_written: u64,
    pub expected_chunks: u64,
    pub out_of_order_count: usize,
    pub buffer_usage: usize,
    pub completed: bool,
    pub cancelled: bool,
    pub progress_percentage: f32,
    pub duration: Duration,
    pub statistics: WriteStatistics,
}

// ENHANCED: Comprehensive resource cleanup with detailed logging
impl Drop for StreamingFileWriter {
    fn drop(&mut self) {
        if !self.completed && !self.cancelled && self.cleanup_on_drop {
            error!(
                "🗑️ StreamingFileWriter dropped unexpectedly: {} ({:.1}MB written of {:.1}MB expected)",
                self.file_path.display(),
                self.bytes_written as f64 / (1024.0 * 1024.0),
                self.expected_size as f64 / (1024.0 * 1024.0)
            );

            // Send failure notification
            if let Some(ref sender) = self.progress_tx {
                let _ = sender.send(ProgressUpdate::Failed {
                    transfer_id: self.transfer_id,
                    error: "Writer dropped before completion".to_string(),
                    bytes_written: self.bytes_written,
                });
            }

            // Clean up partial file if configured
            if self.partial_file_cleanup && self.file_path.exists() {
                if let Err(e) = std::fs::remove_file(&self.file_path) {
                    error!("Failed to clean up incomplete file during drop: {}", e);
                } else {
                    info!(
                        "🗑️ Cleaned up incomplete file during drop: {:?}",
                        self.file_path
                    );
                }
            }

            // Log final statistics for debugging
            let stats = self.get_statistics();
            error!(
                "📊 Drop statistics: {:.1}% complete, {:.1}% efficiency, {} OOO chunks buffered, {:.1}KB compression saves",
                self.progress() * 100.0,
                stats.efficiency_percentage,
                self.out_of_order_chunks.len(),
                self.total_compression_saves as f64 / 1024.0
            );
        } else if self.completed {
            debug!("✅ StreamingFileWriter dropped after successful completion");
        } else if self.cancelled {
            debug!("🛑 StreamingFileWriter dropped after cancellation");
        }
    }
}
