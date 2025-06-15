// src/network/high_speed_memory.rs
use bytes::BytesMut;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

pub struct HighSpeedMemoryPool {
    chunk_buffers: Arc<Mutex<VecDeque<BytesMut>>>,
    batch_buffers: Arc<Mutex<VecDeque<Vec<u8>>>>,

    // Configuration
    max_memory_usage: usize,
    chunk_buffer_size: usize,
    batch_buffer_size: usize,
    max_chunk_buffers: usize,
    max_batch_buffers: usize,

    // Statistics
    current_memory_usage: AtomicU64,
    allocated_chunk_buffers: AtomicUsize,
    allocated_batch_buffers: AtomicUsize,
    cache_hits: AtomicU64,
    cache_misses: AtomicU64,
}

impl HighSpeedMemoryPool {
    pub fn new(max_memory: usize, chunk_size: usize, batch_size: usize) -> Self {
        let max_chunk_buffers = (max_memory / 2) / chunk_size; // 50% for chunk buffers
        let max_batch_buffers = (max_memory / 2) / batch_size; // 50% for batch buffers

        info!("🧠 Creating high-speed memory pool: max_memory={}MB, chunk_buffers={}, batch_buffers={}",
              max_memory / (1024 * 1024), max_chunk_buffers, max_batch_buffers);

        Self {
            chunk_buffers: Arc::new(Mutex::new(VecDeque::new())),
            batch_buffers: Arc::new(Mutex::new(VecDeque::new())),
            max_memory_usage: max_memory,
            chunk_buffer_size: chunk_size,
            batch_buffer_size: batch_size,
            max_chunk_buffers,
            max_batch_buffers,
            current_memory_usage: AtomicU64::new(0),
            allocated_chunk_buffers: AtomicUsize::new(0),
            allocated_batch_buffers: AtomicUsize::new(0),
            cache_hits: AtomicU64::new(0),
            cache_misses: AtomicU64::new(0),
        }
    }

    // Get chunk buffer (for file reading)
    pub async fn get_chunk_buffer(&self) -> BytesMut {
        let mut buffers = self.chunk_buffers.lock().await;

        if let Some(mut buffer) = buffers.pop_front() {
            buffer.clear();
            buffer.reserve(self.chunk_buffer_size);
            self.cache_hits.fetch_add(1, Ordering::Relaxed);
            buffer
        } else {
            // Allocate new buffer if under limit
            let current_chunks = self.allocated_chunk_buffers.load(Ordering::Relaxed);
            if current_chunks < self.max_chunk_buffers {
                let buffer = BytesMut::with_capacity(self.chunk_buffer_size);
                self.allocated_chunk_buffers.fetch_add(1, Ordering::Relaxed);
                self.current_memory_usage
                    .fetch_add(self.chunk_buffer_size as u64, Ordering::Relaxed);
                self.cache_misses.fetch_add(1, Ordering::Relaxed);
                buffer
            } else {
                // Pool exhausted, create temporary buffer
                warn!("⚠️ Chunk buffer pool exhausted, creating temporary buffer");
                BytesMut::with_capacity(self.chunk_buffer_size)
            }
        }
    }

    // Return chunk buffer to pool
    pub async fn return_chunk_buffer(&self, buffer: BytesMut) {
        let mut buffers = self.chunk_buffers.lock().await;

        if buffers.len() < self.max_chunk_buffers && buffer.capacity() >= self.chunk_buffer_size / 2
        {
            buffers.push_back(buffer);
        } else {
            // Pool full or buffer too small, release memory
            self.allocated_chunk_buffers.fetch_sub(1, Ordering::Relaxed);
            self.current_memory_usage
                .fetch_sub(buffer.capacity() as u64, Ordering::Relaxed);
        }
    }

    // Get batch buffer (for network batching)
    pub async fn get_batch_buffer(&self) -> Vec<u8> {
        let mut buffers = self.batch_buffers.lock().await;

        if let Some(mut buffer) = buffers.pop_front() {
            buffer.clear();
            buffer.reserve(self.batch_buffer_size);
            self.cache_hits.fetch_add(1, Ordering::Relaxed);
            buffer
        } else {
            let current_batches = self.allocated_batch_buffers.load(Ordering::Relaxed);
            if current_batches < self.max_batch_buffers {
                let buffer = Vec::with_capacity(self.batch_buffer_size);
                self.allocated_batch_buffers.fetch_add(1, Ordering::Relaxed);
                self.current_memory_usage
                    .fetch_add(self.batch_buffer_size as u64, Ordering::Relaxed);
                self.cache_misses.fetch_add(1, Ordering::Relaxed);
                buffer
            } else {
                warn!("⚠️ Batch buffer pool exhausted, creating temporary buffer");
                Vec::with_capacity(self.batch_buffer_size)
            }
        }
    }

    // Return batch buffer to pool
    pub async fn return_batch_buffer(&self, buffer: Vec<u8>) {
        let mut buffers = self.batch_buffers.lock().await;

        if buffers.len() < self.max_batch_buffers && buffer.capacity() >= self.batch_buffer_size / 2
        {
            buffers.push_back(buffer);
        } else {
            self.allocated_batch_buffers.fetch_sub(1, Ordering::Relaxed);
            self.current_memory_usage
                .fetch_sub(buffer.capacity() as u64, Ordering::Relaxed);
        }
    }

    // Get memory pool statistics
    pub async fn get_stats(&self) -> MemoryPoolStats {
        let chunk_buffers = self.chunk_buffers.lock().await;
        let batch_buffers = self.batch_buffers.lock().await;

        MemoryPoolStats {
            total_memory_usage: self.current_memory_usage.load(Ordering::Relaxed),
            max_memory_usage: self.max_memory_usage as u64,
            chunk_buffers_available: chunk_buffers.len(),
            chunk_buffers_allocated: self.allocated_chunk_buffers.load(Ordering::Relaxed),
            batch_buffers_available: batch_buffers.len(),
            batch_buffers_allocated: self.allocated_batch_buffers.load(Ordering::Relaxed),
            cache_hit_rate: {
                let hits = self.cache_hits.load(Ordering::Relaxed);
                let misses = self.cache_misses.load(Ordering::Relaxed);
                if hits + misses > 0 {
                    hits as f64 / (hits + misses) as f64
                } else {
                    0.0
                }
            },
            memory_pressure: self.current_memory_usage.load(Ordering::Relaxed)
                > (self.max_memory_usage as u64 * 85 / 100), // 85% threshold
        }
    }

    // Cleanup unused buffers
    pub async fn cleanup(&self) {
        let mut chunk_buffers = self.chunk_buffers.lock().await;
        let mut batch_buffers = self.batch_buffers.lock().await;

        let chunks_removed = chunk_buffers.len() / 2; // Remove half
        let batches_removed = batch_buffers.len() / 2;

        for _ in 0..chunks_removed {
            if let Some(buffer) = chunk_buffers.pop_front() {
                self.allocated_chunk_buffers.fetch_sub(1, Ordering::Relaxed);
                self.current_memory_usage
                    .fetch_sub(buffer.capacity() as u64, Ordering::Relaxed);
            }
        }

        for _ in 0..batches_removed {
            if let Some(buffer) = batch_buffers.pop_front() {
                self.allocated_batch_buffers.fetch_sub(1, Ordering::Relaxed);
                self.current_memory_usage
                    .fetch_sub(buffer.capacity() as u64, Ordering::Relaxed);
            }
        }

        info!(
            "🧹 Memory pool cleanup: removed {} chunk buffers, {} batch buffers",
            chunks_removed, batches_removed
        );
    }
}

#[derive(Debug, Clone)]
pub struct MemoryPoolStats {
    pub total_memory_usage: u64,
    pub max_memory_usage: u64,
    pub chunk_buffers_available: usize,
    pub chunk_buffers_allocated: usize,
    pub batch_buffers_available: usize,
    pub batch_buffers_allocated: usize,
    pub cache_hit_rate: f64,
    pub memory_pressure: bool,
}
