use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{info, debug};
use uuid::Uuid;

/// High-performance progress tracking for QUIC file transfers
#[derive(Debug)]
pub struct ProgressTracker {
    transfer_id: Uuid,
    total_chunks: u64,
    completed_chunks: Arc<RwLock<HashSet<u64>>>,
    bytes_transferred: Arc<AtomicU64>,
    start_time: Instant,
    last_update: Arc<RwLock<Instant>>,
    throughput_samples: Arc<RwLock<Vec<ThroughputSample>>>,
    estimated_completion: Arc<RwLock<Option<Instant>>>,
}

#[derive(Debug, Clone)]
pub struct ThroughputSample {
    timestamp: Instant,
    bytes_transferred: u64,
}

#[derive(Debug, Clone)]
pub struct ProgressReport {
    pub transfer_id: Uuid,
    pub progress_percentage: f64,
    pub chunks_completed: u64,
    pub total_chunks: u64,
    pub bytes_transferred: u64,
    pub current_throughput_bps: f64,
    pub average_throughput_bps: f64,
    pub estimated_time_remaining: Option<Duration>,
    pub elapsed_time: Duration,
}

impl ProgressTracker {
    pub fn new(transfer_id: Uuid, total_chunks: u64) -> Self {
        Self {
            transfer_id,
            total_chunks,
            completed_chunks: Arc::new(RwLock::new(HashSet::new())),
            bytes_transferred: Arc::new(AtomicU64::new(0)),
            start_time: Instant::now(),
            last_update: Arc::new(RwLock::new(Instant::now())),
            throughput_samples: Arc::new(RwLock::new(Vec::new())),
            estimated_completion: Arc::new(RwLock::new(None)),
        }
    }

    /// Mark a chunk as completed
    pub async fn update_chunk_completed(&self, chunk_id: u64) {
        {
            let mut completed = self.completed_chunks.write().await;
            if completed.insert(chunk_id) {
                // Only update if this chunk wasn't already completed
                debug!("✅ Chunk {} completed for transfer {}", chunk_id, self.transfer_id);
            }
        }

        // Update last activity time
        {
            let mut last_update = self.last_update.write().await;
            *last_update = Instant::now();
        }

        // Update throughput samples
        self.update_throughput_sample().await;
        
        // Update ETA
        self.update_estimated_completion().await;
    }

    /// Update bytes transferred
    pub async fn update_bytes_transferred(&self, bytes: u64) {
        self.bytes_transferred.store(bytes, Ordering::Relaxed);
        self.update_throughput_sample().await;
        self.update_estimated_completion().await;
    }

    /// Add bytes to the total
    pub async fn add_bytes_transferred(&self, additional_bytes: u64) {
        self.bytes_transferred.fetch_add(additional_bytes, Ordering::Relaxed);
        self.update_throughput_sample().await;
        self.update_estimated_completion().await;
    }

    async fn update_throughput_sample(&self) {
        let now = Instant::now();
        let bytes = self.bytes_transferred.load(Ordering::Relaxed);
        
        let sample = ThroughputSample {
            timestamp: now,
            bytes_transferred: bytes,
        };

        let mut samples = self.throughput_samples.write().await;
        samples.push(sample);

        // Keep only last 60 seconds of samples
        let cutoff = now - Duration::from_secs(60);
        samples.retain(|s| s.timestamp > cutoff);
    }

    async fn update_estimated_completion(&self) {
        let completed_count = {
            let completed = self.completed_chunks.read().await;
            completed.len() as u64
        };

        if completed_count > 0 {
            let elapsed = self.start_time.elapsed();
            let completion_rate = completed_count as f64 / elapsed.as_secs_f64();
            
            if completion_rate > 0.0 {
                let remaining_chunks = self.total_chunks - completed_count;
                let estimated_seconds = remaining_chunks as f64 / completion_rate;
                
                let mut estimated_completion = self.estimated_completion.write().await;
                *estimated_completion = Some(Instant::now() + Duration::from_secs_f64(estimated_seconds));
            }
        }
    }

    /// Get current progress report
    pub async fn get_progress_report(&self) -> ProgressReport {
        let completed_count = {
            let completed = self.completed_chunks.read().await;
            completed.len() as u64
        };

        let bytes_transferred = self.bytes_transferred.load(Ordering::Relaxed);
        let elapsed = self.start_time.elapsed();
        let progress_percentage = if self.total_chunks > 0 {
            (completed_count as f64 / self.total_chunks as f64) * 100.0
        } else {
            0.0
        };

        // Calculate throughput
        let (current_throughput, average_throughput) = self.calculate_throughput().await;

        // Calculate estimated time remaining
        let estimated_time_remaining = {
            let estimated_completion = self.estimated_completion.read().await;
            estimated_completion.map(|completion_time| {
                let now = Instant::now();
                if completion_time > now {
                    completion_time.duration_since(now)
                } else {
                    Duration::from_secs(0)
                }
            })
        };

        ProgressReport {
            transfer_id: self.transfer_id,
            progress_percentage,
            chunks_completed: completed_count,
            total_chunks: self.total_chunks,
            bytes_transferred,
            current_throughput_bps: current_throughput,
            average_throughput_bps: average_throughput,
            estimated_time_remaining,
            elapsed_time: elapsed,
        }
    }

    async fn calculate_throughput(&self) -> (f64, f64) {
        let samples = self.throughput_samples.read().await;
        
        if samples.is_empty() {
            return (0.0, 0.0);
        }

        // Average throughput since start
        let total_bytes = self.bytes_transferred.load(Ordering::Relaxed);
        let elapsed = self.start_time.elapsed();
        let average_throughput = if elapsed.as_secs_f64() > 0.0 {
            total_bytes as f64 / elapsed.as_secs_f64()
        } else {
            0.0
        };

        // Current throughput (last 10 seconds)
        let current_throughput = if samples.len() >= 2 {
            let now = Instant::now();
            let ten_seconds_ago = now - Duration::from_secs(10);
            
            let recent_samples: Vec<_> = samples.iter()
                .filter(|s| s.timestamp > ten_seconds_ago)
                .collect();
            
            if recent_samples.len() >= 2 {
                let first = recent_samples.first().unwrap();
                let last = recent_samples.last().unwrap();
                
                let time_diff = last.timestamp.duration_since(first.timestamp).as_secs_f64();
                let bytes_diff = last.bytes_transferred.saturating_sub(first.bytes_transferred);
                
                if time_diff > 0.0 {
                    bytes_diff as f64 / time_diff
                } else {
                    0.0
                }
            } else {
                average_throughput
            }
        } else {
            average_throughput
        };

        (current_throughput, average_throughput)
    }

    /// Check if transfer is complete
    pub async fn is_complete(&self) -> bool {
        let completed = self.completed_chunks.read().await;
        completed.len() as u64 >= self.total_chunks
    }

    /// Get completion percentage
    pub async fn get_completion_percentage(&self) -> f64 {
        let completed = self.completed_chunks.read().await;
        if self.total_chunks > 0 {
            (completed.len() as f64 / self.total_chunks as f64) * 100.0
        } else {
            0.0
        }
    }

    /// Get missing chunks
    pub async fn get_missing_chunks(&self) -> Vec<u64> {
        let completed = self.completed_chunks.read().await;
        let all_chunks: HashSet<u64> = (0..self.total_chunks).collect();
        let missing: Vec<u64> = all_chunks.difference(&completed).cloned().collect();
        missing
    }

    /// Get throughput in MB/s
    pub async fn get_throughput_mbps(&self) -> f64 {
        let (current_throughput, _) = self.calculate_throughput().await;
        current_throughput / (1024.0 * 1024.0)
    }

    /// Reset progress (for retries)
    pub async fn reset(&self) {
        {
            let mut completed = self.completed_chunks.write().await;
            completed.clear();
        }
        
        self.bytes_transferred.store(0, Ordering::Relaxed);
        
        {
            let mut last_update = self.last_update.write().await;
            *last_update = Instant::now();
        }
        
        {
            let mut samples = self.throughput_samples.write().await;
            samples.clear();
        }
        
        {
            let mut estimated_completion = self.estimated_completion.write().await;
            *estimated_completion = None;
        }

        info!("🔄 Reset progress for transfer {}", self.transfer_id);
    }

    /// Log progress (for debugging)
    pub async fn log_progress(&self) {
        let report = self.get_progress_report().await;
        
        info!("📊 Transfer {} Progress: {:.1}% ({}/{} chunks) - {:.1} MB/s current, {:.1} MB/s average - ETA: {}",
              self.transfer_id,
              report.progress_percentage,
              report.chunks_completed,
              report.total_chunks,
              report.current_throughput_bps / (1024.0 * 1024.0),
              report.average_throughput_bps / (1024.0 * 1024.0),
              if let Some(eta) = report.estimated_time_remaining {
                  format!("{:.1}s", eta.as_secs_f64())
              } else {
                  "unknown".to_string()
              }
        );
    }
}

impl Clone for ProgressTracker {
    fn clone(&self) -> Self {
        Self {
            transfer_id: self.transfer_id,
            total_chunks: self.total_chunks,
            completed_chunks: self.completed_chunks.clone(),
            bytes_transferred: self.bytes_transferred.clone(),
            start_time: self.start_time,
            last_update: self.last_update.clone(),
            throughput_samples: self.throughput_samples.clone(),
            estimated_completion: self.estimated_completion.clone(),
        }
    }
}