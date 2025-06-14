use crate::network::streaming_protocol::*;
use crate::{FileshareError, Result};
use bytes::{BufMut, Bytes, BytesMut};
use lz4_flex::compress_prepend_size;
use memmap2::{Mmap, MmapOptions};
use std::collections::VecDeque;
use std::fs::File;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

// ENHANCED: System-aware memory mapping strategy with resource monitoring
pub struct MemoryMappingStrategy {
    total_system_memory: u64,
    available_memory: u64,
    max_mmap_ratio: f64,
    last_check: Instant,
    check_interval: std::time::Duration,
}

impl MemoryMappingStrategy {
    pub fn new() -> Self {
        let (total, available) = Self::get_system_memory();
        Self {
            total_system_memory: total,
            available_memory: available,
            max_mmap_ratio: 0.25, // Use max 25% of available memory
            last_check: Instant::now(),
            check_interval: std::time::Duration::from_secs(30), // Check every 30 seconds
        }
    }

    fn get_system_memory() -> (u64, u64) {
        #[cfg(target_os = "linux")]
        {
            if let Ok(info) = sys_info::mem_info() {
                return (info.total * 1024, info.avail * 1024); // Convert from KB to bytes
            }
        }

        #[cfg(target_os = "windows")]
        {
            if let Ok(info) = sys_info::mem_info() {
                return (info.total * 1024, info.avail * 1024);
            }
        }

        #[cfg(target_os = "macos")]
        {
            if let Ok(info) = sys_info::mem_info() {
                return (info.total * 1024, info.avail * 1024);
            }
        }

        // Fallback - conservative estimate
        (8 * 1024 * 1024 * 1024, 2 * 1024 * 1024 * 1024) // 8GB total, 2GB available
    }

    pub fn update_memory_info(&mut self) {
        if self.last_check.elapsed() > self.check_interval {
            let (total, available) = Self::get_system_memory();
            self.total_system_memory = total;
            self.available_memory = available;
            self.last_check = Instant::now();

            debug!(
                "Updated memory info: {}GB total, {}GB available",
                total / (1024 * 1024 * 1024),
                available / (1024 * 1024 * 1024)
            );
        }
    }

    pub fn should_use_mmap(&mut self, file_size: u64) -> bool {
        self.update_memory_info();

        const MIN_MMAP_SIZE: u64 = 4 * 1024 * 1024; // 4MB minimum
        const MAX_MMAP_SIZE: u64 = 4 * 1024 * 1024 * 1024; // 4GB maximum

        // Too small for mmap overhead
        if file_size < MIN_MMAP_SIZE {
            return false;
        }

        // Too large for safe mmap
        if file_size > MAX_MMAP_SIZE {
            return false;
        }

        // Check available memory with safety margin
        let max_mmap_size = (self.available_memory as f64 * self.max_mmap_ratio) as u64;
        let should_mmap = file_size <= max_mmap_size;

        info!(
            "Memory mapping decision: file_size={}MB, available={}MB, max_mmap={}MB, use_mmap={}",
            file_size / (1024 * 1024),
            self.available_memory / (1024 * 1024),
            max_mmap_size / (1024 * 1024),
            should_mmap
        );

        should_mmap
    }

    pub fn get_optimal_chunk_size(&self, file_size: u64, base_chunk_size: usize) -> usize {
        let min_chunk_size = 64 * 1024; // 64KB minimum
        let max_chunk_size = 8 * 1024 * 1024; // 8MB maximum

        // Memory-based scaling
        let memory_gb = self.available_memory / (1024 * 1024 * 1024);
        let memory_factor = (memory_gb as f64 / 4.0).min(2.0); // Scale up to 2x for 4GB+ memory

        // File size-based scaling
        let file_mb = file_size / (1024 * 1024);
        let size_factor = match file_mb {
            0..=10 => 0.5,     // Small files: smaller chunks for responsiveness
            11..=100 => 1.0,   // Medium files: base chunk size
            101..=1000 => 1.5, // Large files: larger chunks for efficiency
            _ => 2.0,          // Very large files: maximum efficiency
        };

        let adaptive_size = (base_chunk_size as f64 * memory_factor * size_factor) as usize;
        adaptive_size.clamp(min_chunk_size, max_chunk_size)
    }
}

// ENHANCED: Zero-copy buffer pool with memory pressure handling
pub struct BufferPool {
    buffers: Arc<Mutex<VecDeque<BytesMut>>>,
    buffer_size: usize,
    max_buffers: usize,
    current_buffers: Arc<std::sync::atomic::AtomicUsize>,
    total_allocated: Arc<std::sync::atomic::AtomicU64>,
    memory_pressure_threshold: u64,
}

impl BufferPool {
    pub fn new(buffer_size: usize, max_buffers: usize) -> Self {
        let memory_pressure_threshold = max_buffers as u64 * buffer_size as u64 * 2; // 2x normal usage

        Self {
            buffers: Arc::new(Mutex::new(VecDeque::with_capacity(max_buffers))),
            buffer_size,
            max_buffers,
            current_buffers: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            total_allocated: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            memory_pressure_threshold,
        }
    }

    pub async fn get_buffer(&self) -> BytesMut {
        let mut buffers = self.buffers.lock().await;

        if let Some(mut buffer) = buffers.pop_front() {
            buffer.clear();
            buffer.reserve(self.buffer_size);
            debug!("Reused buffer from pool, {} remaining", buffers.len());
            buffer
        } else {
            // Check memory pressure before allocating
            let current_usage = self
                .total_allocated
                .load(std::sync::atomic::Ordering::Relaxed);
            if current_usage > self.memory_pressure_threshold {
                warn!("Memory pressure detected, forcing garbage collection");
                // Force a small delay to allow GC
                tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
            }

            let buffer = BytesMut::with_capacity(self.buffer_size);
            self.current_buffers
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.total_allocated.fetch_add(
                self.buffer_size as u64,
                std::sync::atomic::Ordering::Relaxed,
            );

            debug!(
                "Allocated new buffer, total buffers: {}, total memory: {}MB",
                self.current_buffers
                    .load(std::sync::atomic::Ordering::Relaxed),
                self.total_allocated
                    .load(std::sync::atomic::Ordering::Relaxed)
                    / (1024 * 1024)
            );

            buffer
        }
    }

    pub async fn return_buffer(&self, buffer: BytesMut) {
        let mut buffers = self.buffers.lock().await;

        if buffers.len() < self.max_buffers && buffer.capacity() >= self.buffer_size / 2 {
            buffers.push_back(buffer);
            debug!("Returned buffer to pool, {} total", buffers.len());
        } else {
            // Pool is full or buffer is too small, drop it
            self.current_buffers
                .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
            self.total_allocated.fetch_sub(
                buffer.capacity() as u64,
                std::sync::atomic::Ordering::Relaxed,
            );
            debug!("Dropped buffer, pool full or buffer too small");
        }
    }

    pub fn get_stats(&self) -> BufferPoolStats {
        BufferPoolStats {
            buffer_size: self.buffer_size,
            max_buffers: self.max_buffers,
            current_buffers: self
                .current_buffers
                .load(std::sync::atomic::Ordering::Relaxed),
            total_allocated_bytes: self
                .total_allocated
                .load(std::sync::atomic::Ordering::Relaxed),
            memory_pressure: self
                .total_allocated
                .load(std::sync::atomic::Ordering::Relaxed)
                > self.memory_pressure_threshold,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BufferPoolStats {
    pub buffer_size: usize,
    pub max_buffers: usize,
    pub current_buffers: usize,
    pub total_allocated_bytes: u64,
    pub memory_pressure: bool,
}

// ENHANCED: Compression task management
#[derive(Debug)]
pub struct CompressionTask {
    pub chunk_index: u64,
    pub task: JoinHandle<Result<CompressionResult>>,
    pub started_at: Instant,
}

#[derive(Debug)]
pub struct CompressionResult {
    pub compressed: bool,
    pub data: Bytes,
    pub original_size: usize,
    pub compressed_size: usize,
    pub compression_time: std::time::Duration,
}

// ENHANCED: Main streaming file reader with intelligent method selection
pub struct StreamingFileReader {
    config: StreamingConfig,
    file_path: PathBuf,
    file_size: u64,
    transfer_id: Uuid,
    memory_strategy: MemoryMappingStrategy,
    optimal_chunk_size: usize,
    buffer_pool: Arc<BufferPool>,
    performance_metrics: ReaderMetrics,
}

#[derive(Debug, Default)]
pub struct ReaderMetrics {
    pub bytes_read: u64,
    pub chunks_generated: u64,
    pub compression_ratio: f64,
    pub zero_copy_chunks: u64,
    pub copied_chunks: u64,
    pub compression_time: std::time::Duration,
    pub read_time: std::time::Duration,
    pub started_at: Option<Instant>,
}

impl StreamingFileReader {
    pub async fn new(
        file_path: PathBuf,
        transfer_id: Uuid,
        config: StreamingConfig,
    ) -> Result<Self> {
        // Validate config first
        config
            .validate()
            .map_err(|e| FileshareError::Transfer(e.to_string()))?;

        let file = File::open(&file_path)
            .map_err(|e| FileshareError::FileOperation(format!("Cannot open file: {}", e)))?;

        let file_size = file
            .metadata()
            .map_err(|e| FileshareError::FileOperation(format!("Cannot get file metadata: {}", e)))?
            .len();

        if file_size == 0 {
            return Err(FileshareError::FileOperation(
                "Cannot read empty file".to_string(),
            ));
        }

        let mut memory_strategy = MemoryMappingStrategy::new();
        let optimal_chunk_size =
            memory_strategy.get_optimal_chunk_size(file_size, config.base_chunk_size);

        // Create appropriately sized buffer pool
        let pool_size = std::cmp::min(
            config.max_concurrent_chunks * 2, // 2x for double buffering
            16,                               // Maximum 16 buffers
        );
        let buffer_pool = Arc::new(BufferPool::new(optimal_chunk_size * 2, pool_size));

        info!(
            "🚀 StreamingFileReader initialized: {} ({:.1}MB)",
            file_path.display(),
            file_size as f64 / (1024.0 * 1024.0)
        );
        info!(
            "   Chunk size: {}KB, Buffer pool: {} buffers, Zero-copy threshold: {}MB",
            optimal_chunk_size / 1024,
            pool_size,
            config.zero_copy_threshold / (1024 * 1024)
        );

        Ok(Self {
            config,
            file_path,
            file_size,
            transfer_id,
            memory_strategy,
            optimal_chunk_size,
            buffer_pool,
            performance_metrics: ReaderMetrics::default(),
        })
    }

    pub fn file_size(&self) -> u64 {
        self.file_size
    }

    pub fn estimated_chunks(&self) -> u64 {
        (self.file_size + self.optimal_chunk_size as u64 - 1) / self.optimal_chunk_size as u64
    }

    pub fn should_use_zero_copy(&mut self) -> bool {
        self.file_size > self.config.zero_copy_threshold as u64
            && self.memory_strategy.should_use_mmap(self.file_size)
    }

    pub async fn create_chunk_stream(mut self) -> Result<StreamingChunkReader> {
        self.performance_metrics.started_at = Some(Instant::now());

        if self.should_use_zero_copy() {
            info!("🚀 Using zero-copy memory-mapped streaming");
            self.create_zero_copy_mmap_stream().await
        } else {
            info!("📦 Using optimized buffered streaming");
            self.create_optimized_buffered_stream().await
        }
    }

    async fn create_zero_copy_mmap_stream(self) -> Result<StreamingChunkReader> {
        let file = File::open(&self.file_path)
            .map_err(|e| FileshareError::FileOperation(format!("Cannot open file: {}", e)))?;

        let mmap = unsafe {
            let mut options = MmapOptions::new();
            options.populate(); // Pre-populate pages for better performance

            // Try to use huge pages on Linux for better performance
            #[cfg(target_os = "linux")]
            {
                if let Err(e) = options.huge(Some(memmap2::Advice::Hugepage)) {
                    debug!("Could not enable huge pages: {}", e);
                }
            }

            options
                .map(&file)
                .map_err(|e| FileshareError::FileOperation(format!("Cannot mmap file: {}", e)))?
        };

        info!(
            "🚀 ZERO_COPY: Memory-mapped {:.1}MB with optimized settings",
            mmap.len() as f64 / (1024.0 * 1024.0)
        );

        Ok(StreamingChunkReader::new_zero_copy_mmap(
            Arc::new(mmap),
            self.transfer_id,
            self.config,
            self.optimal_chunk_size,
            self.buffer_pool,
            self.performance_metrics,
        ))
    }

    async fn create_optimized_buffered_stream(self) -> Result<StreamingChunkReader> {
        let file = tokio::fs::File::open(&self.file_path)
            .await
            .map_err(|e| FileshareError::FileOperation(format!("Cannot open file: {}", e)))?;

        Ok(StreamingChunkReader::new_optimized_buffered(
            file,
            self.transfer_id,
            self.config,
            self.optimal_chunk_size,
            self.buffer_pool,
            self.performance_metrics,
        ))
    }

    pub fn get_metrics(&self) -> &ReaderMetrics {
        &self.performance_metrics
    }
}

// ENHANCED: High-performance chunk reader with multiple strategies
pub enum StreamingChunkReader {
    ZeroCopyMmap {
        mmap: Arc<Mmap>,
        transfer_id: Uuid,
        config: StreamingConfig,
        current_offset: usize,
        total_size: usize,
        current_chunk: u64,
        chunk_size: usize,
        buffer_pool: Arc<BufferPool>,
        compression_tasks: Vec<CompressionTask>,
        metrics: ReaderMetrics,
        max_pending_compressions: usize,
    },
    OptimizedBuffered {
        file: tokio::fs::File,
        transfer_id: Uuid,
        config: StreamingConfig,
        current_chunk: u64,
        chunk_size: usize,
        read_buffer: BytesMut,
        buffer_pool: Arc<BufferPool>,
        prefetch_buffer: Option<BytesMut>,
        prefetch_task: Option<JoinHandle<Result<BytesMut>>>,
        metrics: ReaderMetrics,
        file_size: Option<u64>,
    },
}

impl StreamingChunkReader {
    fn new_zero_copy_mmap(
        mmap: Arc<Mmap>,
        transfer_id: Uuid,
        config: StreamingConfig,
        chunk_size: usize,
        buffer_pool: Arc<BufferPool>,
        metrics: ReaderMetrics,
    ) -> Self {
        let total_size = mmap.len();
        let max_pending_compressions = std::cmp::min(config.max_concurrent_chunks, 4);

        Self::ZeroCopyMmap {
            mmap,
            transfer_id,
            config,
            current_offset: 0,
            total_size,
            current_chunk: 0,
            chunk_size,
            buffer_pool,
            compression_tasks: Vec::new(),
            metrics,
            max_pending_compressions,
        }
    }

    fn new_optimized_buffered(
        file: tokio::fs::File,
        transfer_id: Uuid,
        config: StreamingConfig,
        chunk_size: usize,
        buffer_pool: Arc<BufferPool>,
        metrics: ReaderMetrics,
    ) -> Self {
        Self::OptimizedBuffered {
            file,
            transfer_id,
            config,
            current_chunk: 0,
            chunk_size,
            read_buffer: BytesMut::with_capacity(chunk_size),
            buffer_pool,
            prefetch_buffer: None,
            prefetch_task: None,
            metrics,
            file_size: None,
        }
    }

    pub async fn next_chunk(&mut self) -> Result<Option<(StreamChunkHeader, Bytes)>> {
        match self {
            Self::ZeroCopyMmap {
                mmap,
                transfer_id,
                config,
                current_offset,
                total_size,
                current_chunk,
                chunk_size,
                buffer_pool,
                compression_tasks,
                metrics,
                max_pending_compressions,
            } => {
                // Check for completed compression tasks first
                let mut completed_task_index = None;
                for (i, task) in compression_tasks.iter().enumerate() {
                    if task.task.is_finished() {
                        completed_task_index = Some(i);
                        break;
                    }
                }

                if let Some(index) = completed_task_index {
                    let task = compression_tasks.remove(index);
                    match task.task.await {
                        Ok(Ok(result)) => {
                            let mut header = StreamChunkHeader::new(
                                *transfer_id,
                                task.chunk_index,
                                result.compressed_size as u32,
                            );
                            if result.compressed {
                                header.set_compressed();
                                metrics.compression_time += result.compression_time;
                            }
                            header.calculate_checksum(&result.data);

                            debug!(
                                "📦 ASYNC_COMPRESSED chunk {}: {} bytes ({}% compression)",
                                task.chunk_index,
                                result.compressed_size,
                                if result.compressed {
                                    ((result.original_size - result.compressed_size) * 100
                                        / result.original_size)
                                } else {
                                    0
                                }
                            );

                            return Ok(Some((header, result.data)));
                        }
                        Ok(Err(e)) => {
                            error!("Compression task failed: {}", e);
                        }
                        Err(e) => {
                            error!("Compression task panicked: {}", e);
                        }
                    }
                }

                // Generate new chunk if we haven't reached the end
                if *current_offset >= *total_size {
                    // Wait for any remaining compression tasks
                    while let Some(task) = compression_tasks.pop() {
                        if let Ok(Ok(result)) = task.task.await {
                            let mut header = StreamChunkHeader::new(
                                *transfer_id,
                                task.chunk_index,
                                result.compressed_size as u32,
                            );
                            if result.compressed {
                                header.set_compressed();
                            }
                            header.calculate_checksum(&result.data);
                            return Ok(Some((header, result.data)));
                        }
                    }
                    return Ok(None);
                }

                let read_start = Instant::now();
                let actual_chunk_size = std::cmp::min(*chunk_size, *total_size - *current_offset);
                let chunk_slice = &mmap[*current_offset..*current_offset + actual_chunk_size];
                let is_last = *current_offset + actual_chunk_size >= *total_size;
                metrics.read_time += read_start.elapsed();

                let mut header =
                    StreamChunkHeader::new(*transfer_id, *current_chunk, actual_chunk_size as u32);
                if is_last {
                    header.set_last_chunk();
                }

                // Decide on processing strategy
                let should_compress = config.enable_compression
                    && chunk_slice.len() > config.compression_threshold
                    && chunk_slice.len() > 32 * 1024; // Only compress chunks > 32KB

                let final_data =
                    if should_compress && compression_tasks.len() < *max_pending_compressions {
                        // Start async compression
                        let chunk_data = chunk_slice.to_vec(); // Copy for async processing
                        let compression_level = config.compression_level;
                        let task_chunk_index = *current_chunk;

                        let compression_task = tokio::task::spawn_blocking(move || {
                            let compression_start = Instant::now();
                            let original_size = chunk_data.len();
                            let compressed = compress_prepend_size(&chunk_data);
                            let compression_time = compression_start.elapsed();

                            let (is_compressed, final_data, final_size) =
                                if compressed.len() < chunk_data.len() {
                                    let len = compressed.len();
                                    (true, Bytes::from(compressed), len)
                                } else {
                                    let len = chunk_data.len();
                                    (false, Bytes::from(chunk_data), len)
                                };

                            Ok(CompressionResult {
                                compressed: is_compressed,
                                data: final_data,
                                original_size,
                                compressed_size: final_size,
                                compression_time,
                            })
                        });

                        compression_tasks.push(CompressionTask {
                            chunk_index: task_chunk_index,
                            task: compression_task,
                            started_at: Instant::now(),
                        });

                        // For the current chunk, use zero-copy if possible
                        if chunk_slice.len() <= 64 * 1024 {
                            // Small chunks: return immediately without compression
                            metrics.zero_copy_chunks += 1;
                            Bytes::copy_from_slice(chunk_slice)
                        } else {
                            // Large chunks: quick compress synchronously for this chunk
                            let compressed = compress_prepend_size(chunk_slice);
                            if compressed.len() < chunk_slice.len() {
                                header.set_compressed();
                                header.chunk_size = compressed.len() as u32;
                                Bytes::from(compressed)
                            } else {
                                Bytes::copy_from_slice(chunk_slice)
                            }
                        }
                    } else {
                        // Zero-copy path for uncompressed data
                        metrics.zero_copy_chunks += 1;
                        Bytes::copy_from_slice(chunk_slice)
                    };

                header.calculate_checksum(&final_data);
                metrics.bytes_read += actual_chunk_size as u64;
                metrics.chunks_generated += 1;

                *current_offset += actual_chunk_size;
                *current_chunk += 1;

                debug!(
                    "📦 ZERO_COPY chunk {}: {} bytes {} (progress: {:.1}%)",
                    header.get_chunk_index(),
                    final_data.len(),
                    if header.is_compressed() {
                        "compressed"
                    } else {
                        "zero-copy"
                    },
                    (*current_offset as f64 / *total_size as f64) * 100.0
                );

                Ok(Some((header, final_data)))
            }

            Self::OptimizedBuffered {
                file,
                transfer_id,
                config,
                current_chunk,
                chunk_size,
                read_buffer,
                buffer_pool,
                prefetch_buffer,
                prefetch_task,
                metrics,
                file_size,
            } => {
                // Handle prefetched data
                if let Some(prefetch_buf) = prefetch_buffer.take() {
                    std::mem::swap(read_buffer, &mut prefetch_buffer.insert(prefetch_buf));
                } else if let Some(task) = prefetch_task.take() {
                    // Wait for prefetch task to complete
                    match task.await {
                        Ok(Ok(buf)) => {
                            *read_buffer = buf;
                        }
                        Ok(Err(e)) => {
                            warn!("Prefetch task failed: {}", e);
                            // Fall back to synchronous read
                            read_buffer.clear();
                            read_buffer.resize(*chunk_size, 0);
                            let bytes_read = file.read(read_buffer).await?;
                            if bytes_read == 0 {
                                return Ok(None);
                            }
                            read_buffer.truncate(bytes_read);
                        }
                        Err(e) => {
                            error!("Prefetch task panicked: {}", e);
                            return Ok(None);
                        }
                    }
                } else {
                    // Synchronous read
                    let read_start = Instant::now();
                    read_buffer.clear();
                    read_buffer.resize(*chunk_size, 0);

                    let bytes_read = file.read(read_buffer).await?;
                    metrics.read_time += read_start.elapsed();

                    if bytes_read == 0 {
                        return Ok(None);
                    }

                    read_buffer.truncate(bytes_read);
                }

                // Start prefetching next chunk
                if prefetch_task.is_none() {
                    let mut file_clone = file.try_clone().await?;
                    let chunk_size_clone = *chunk_size;
                    let buffer_pool_clone = buffer_pool.clone();

                    *prefetch_task = Some(tokio::spawn(async move {
                        let mut prefetch_buf = buffer_pool_clone.get_buffer().await;
                        prefetch_buf.clear();
                        prefetch_buf.resize(chunk_size_clone, 0);

                        match file_clone.read(&mut prefetch_buf).await {
                            Ok(bytes_read) => {
                                prefetch_buf.truncate(bytes_read);
                                Ok(prefetch_buf)
                            }
                            Err(e) => Err(FileshareError::FileOperation(format!(
                                "Prefetch error: {}",
                                e
                            ))),
                        }
                    }));
                }

                let mut header =
                    StreamChunkHeader::new(*transfer_id, *current_chunk, read_buffer.len() as u32);

                // Check if this is the last chunk
                if file_size.is_none() {
                    if let Ok(metadata) = file.metadata().await {
                        *file_size = Some(metadata.len());
                    }
                }

                let current_pos = file.stream_position().await?;
                let is_last = if let Some(size) = file_size {
                    current_pos >= *size
                } else {
                    false // Can't determine, assume not last
                };

                if is_last {
                    header.set_last_chunk();
                }

                // Compression with reusable buffers
                let final_data = if config.enable_compression
                    && read_buffer.len() > config.compression_threshold
                {
                    let compression_start = Instant::now();
                    let data_to_compress = read_buffer.to_vec();

                    let compressed = tokio::task::spawn_blocking(move || {
                        compress_prepend_size(&data_to_compress)
                    })
                    .await
                    .map_err(|e| {
                        FileshareError::Unknown(format!("Compression task failed: {}", e))
                    })?;

                    metrics.compression_time += compression_start.elapsed();

                    if compressed.len() < read_buffer.len() {
                        header.set_compressed();
                        header.chunk_size = compressed.len() as u32;
                        metrics.compression_ratio =
                            compressed.len() as f64 / read_buffer.len() as f64;
                        Bytes::from(compressed)
                    } else {
                        metrics.copied_chunks += 1;
                        read_buffer.clone().freeze()
                    }
                } else {
                    metrics.copied_chunks += 1;
                    read_buffer.clone().freeze()
                };

                header.calculate_checksum(&final_data);
                metrics.bytes_read += read_buffer.len() as u64;
                metrics.chunks_generated += 1;
                *current_chunk += 1;

                debug!(
                    "📦 BUFFERED chunk {}: {} bytes {} (last: {})",
                    header.get_chunk_index(),
                    final_data.len(),
                    if header.is_compressed() {
                        "compressed"
                    } else {
                        "uncompressed"
                    },
                    is_last
                );

                Ok(Some((header, final_data)))
            }
        }
    }

    pub fn progress(&self) -> f32 {
        match self {
            Self::ZeroCopyMmap {
                current_offset,
                total_size,
                ..
            } => *current_offset as f32 / *total_size as f32,
            Self::OptimizedBuffered {
                current_chunk,
                file_size,
                ..
            } => {
                if let Some(size) = file_size {
                    let chunk_size = match self {
                        Self::OptimizedBuffered { chunk_size, .. } => *chunk_size,
                        _ => unreachable!(),
                    };
                    let estimated_total_chunks =
                        (*size + chunk_size as u64 - 1) / chunk_size as u64;
                    (*current_chunk as f32 / estimated_total_chunks as f32).min(1.0)
                } else {
                    0.0
                }
            }
        }
    }

    pub fn get_metrics(&self) -> &ReaderMetrics {
        match self {
            Self::ZeroCopyMmap { metrics, .. } => metrics,
            Self::OptimizedBuffered { metrics, .. } => metrics,
        }
    }

    // Cancel any pending operations
    pub fn cancel(&mut self) {
        match self {
            Self::ZeroCopyMmap {
                compression_tasks, ..
            } => {
                for task in compression_tasks.drain(..) {
                    task.task.abort();
                    debug!("Cancelled compression task for chunk {}", task.chunk_index);
                }
            }
            Self::OptimizedBuffered { prefetch_task, .. } => {
                if let Some(task) = prefetch_task.take() {
                    task.abort();
                    debug!("Cancelled prefetch task");
                }
            }
        }
    }
}

// ENHANCED: Proper cleanup with buffer return
impl Drop for StreamingChunkReader {
    fn drop(&mut self) {
        self.cancel(); // Cancel any pending operations

        match self {
            Self::ZeroCopyMmap { buffer_pool, .. } => {
                debug!("Dropped ZeroCopyMmap reader");
                // Buffer pool will clean up automatically
            }
            Self::OptimizedBuffered {
                buffer_pool,
                read_buffer,
                prefetch_buffer,
                ..
            } => {
                // Return buffers to pool asynchronously
                let pool = buffer_pool.clone();
                let read_buf = std::mem::take(read_buffer);
                let prefetch_buf = prefetch_buffer.take();

                tokio::spawn(async move {
                    pool.return_buffer(read_buf).await;
                    if let Some(buf) = prefetch_buf {
                        pool.return_buffer(buf).await;
                    }
                    debug!("Returned buffers to pool");
                });
            }
        }
    }
}
