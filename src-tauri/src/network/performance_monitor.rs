// src/network/performance_monitor.rs
use super::high_speed_streaming::HighSpeedConfig;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, info};

pub struct PerformanceMonitor {
    config: HighSpeedConfig,
    metrics: Arc<RwLock<PerformanceMetrics>>,
    connection_metrics: Arc<RwLock<Vec<ConnectionMetrics>>>,
    start_time: Instant,
}

#[derive(Debug, Clone)]
pub struct PerformanceMetrics {
    pub bytes_transferred: u64,
    pub chunks_processed: u64,
    pub batches_sent: u64,
    pub compression_ratio: f64,
    pub cpu_usage: f64,
    pub memory_usage: u64,
    pub network_utilization: f64,
    pub speed_history: VecDeque<SpeedSample>,
    pub error_count: u64,
    pub last_update: Instant,
}

#[derive(Debug)]
pub struct ConnectionMetrics {
    pub connection_id: usize,
    pub bytes_sent: AtomicU64,
    pub packets_sent: AtomicU64,
    pub last_activity: Instant,
    pub health_score: f64,
    pub speed_mbps: f64,
}

#[derive(Debug, Clone)]
pub struct SpeedSample {
    pub timestamp: Instant,
    pub bytes_per_second: f64,
    pub cpu_usage: f64,
}

impl PerformanceMonitor {
    pub fn new(config: HighSpeedConfig) -> Self {
        let connection_count = config.connection_count;

        let mut connection_metrics = Vec::new();
        for i in 0..connection_count {
            connection_metrics.push(ConnectionMetrics {
                connection_id: i,
                bytes_sent: AtomicU64::new(0),
                packets_sent: AtomicU64::new(0),
                last_activity: Instant::now(),
                health_score: 1.0,
                speed_mbps: 0.0,
            });
        }

        Self {
            config,
            metrics: Arc::new(RwLock::new(PerformanceMetrics {
                bytes_transferred: 0,
                chunks_processed: 0,
                batches_sent: 0,
                compression_ratio: 1.0,
                cpu_usage: 0.0,
                memory_usage: 0,
                network_utilization: 0.0,
                speed_history: VecDeque::with_capacity(60), // 60 seconds of history
                error_count: 0,
                last_update: Instant::now(),
            })),
            connection_metrics: Arc::new(RwLock::new(connection_metrics)),
            start_time: Instant::now(),
        }
    }

    // Record bytes sent on a specific connection
    pub async fn record_bytes_sent(&self, connection_id: usize, bytes: u64) {
        {
            let mut connections = self.connection_metrics.write().await;
            if let Some(conn) = connections.get_mut(connection_id) {
                conn.bytes_sent.fetch_add(bytes, Ordering::Relaxed);
                conn.packets_sent.fetch_add(1, Ordering::Relaxed);
                conn.last_activity = Instant::now();
            }
        }

        // Update overall metrics
        let mut metrics = self.metrics.write().await;
        metrics.bytes_transferred += bytes;
        metrics.batches_sent += 1;

        // Update speed history every second
        let now = Instant::now();
        if now.duration_since(metrics.last_update) >= Duration::from_secs(1) {
            let elapsed = now.duration_since(metrics.last_update).as_secs_f64();
            let bytes_per_second = bytes as f64 / elapsed;
            let cpu_usage = self.get_cpu_usage().await;

            metrics.speed_history.push_back(SpeedSample {
                timestamp: now,
                bytes_per_second,
                cpu_usage,
            });

            // Keep only last 60 samples (1 minute)
            if metrics.speed_history.len() > 60 {
                metrics.speed_history.pop_front();
            }

            metrics.cpu_usage = cpu_usage;
            metrics.last_update = now;
        }
    }

    // Get current performance metrics
    pub async fn get_current_metrics(&self) -> PerformanceMetrics {
        let metrics = self.metrics.read().await;
        metrics.clone()
    }

    // Calculate current transfer speed
    pub async fn calculate_current_speed_mbps(&self) -> f64 {
        let metrics = self.metrics.read().await;

        if metrics.speed_history.len() < 2 {
            return 0.0;
        }

        // Calculate average speed over last 5 samples
        let samples_to_use = std::cmp::min(5, metrics.speed_history.len());
        let recent_samples: Vec<_> = metrics
            .speed_history
            .iter()
            .rev()
            .take(samples_to_use)
            .collect();

        let total_bytes_per_second: f64 = recent_samples.iter().map(|s| s.bytes_per_second).sum();

        let avg_bytes_per_second = total_bytes_per_second / recent_samples.len() as f64;
        avg_bytes_per_second / (1024.0 * 1024.0) // Convert to MB/s
    }

    // Calculate per-connection speeds
    pub async fn calculate_connection_speeds(&self) -> Vec<f64> {
        let connections = self.connection_metrics.read().await;
        let mut speeds = Vec::new();

        for conn in connections.iter() {
            let bytes_sent = conn.bytes_sent.load(Ordering::Relaxed);
            let elapsed = conn.last_activity.elapsed().as_secs_f64();

            let speed_mbps = if elapsed > 0.0 {
                (bytes_sent as f64) / (elapsed * 1024.0 * 1024.0)
            } else {
                0.0
            };

            speeds.push(speed_mbps);
        }

        speeds
    }

    // Monitor and adjust performance in real-time
    pub async fn auto_tune_performance(&self) -> Option<PerformanceTuning> {
        let metrics = self.metrics.read().await;

        if metrics.speed_history.len() < 10 {
            return None; // Not enough data
        }

        let current_speed = self.calculate_current_speed_mbps().await;
        let cpu_usage = metrics.cpu_usage;
        let target_speed = self.config.bandwidth_target_mbps;

        // Performance tuning recommendations
        let mut tuning = PerformanceTuning::default();

        // If we're below target speed and CPU usage is low, increase parallelism
        if current_speed < target_speed * 0.8 && cpu_usage < 0.6 {
            tuning.increase_chunk_size = true;
            tuning.increase_batch_size = true;
            tuning.suggested_chunk_size = Some(self.config.chunk_size * 2);
        }

        // If CPU usage is too high, reduce compression or chunk size
        if cpu_usage > 0.9 {
            tuning.reduce_compression = true;
            tuning.suggested_chunk_size = Some(self.config.chunk_size / 2);
        }

        // If we're getting good speed, maintain current settings
        if current_speed >= target_speed * 0.9 && cpu_usage < 0.8 {
            tuning.maintain_current = true;
        }

        Some(tuning)
    }

    // Get CPU usage (platform-specific)
    async fn get_cpu_usage(&self) -> f64 {
        // This is a simplified version - in production you'd use platform-specific APIs
        #[cfg(target_os = "linux")]
        {
            self.get_cpu_usage_linux().await
        }
        #[cfg(target_os = "windows")]
        {
            self.get_cpu_usage_windows().await
        }
        #[cfg(target_os = "macos")]
        {
            self.get_cpu_usage_macos().await
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
        {
            0.0 // Fallback
        }
    }

    #[cfg(target_os = "linux")]
    async fn get_cpu_usage_linux(&self) -> f64 {
        // Read /proc/stat to get CPU usage
        match tokio::fs::read_to_string("/proc/stat").await {
            Ok(content) => {
                if let Some(line) = content.lines().next() {
                    // Parse CPU line: cpu user nice system idle iowait irq softirq
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 8 {
                        let user: u64 = parts[1].parse().unwrap_or(0);
                        let nice: u64 = parts[2].parse().unwrap_or(0);
                        let system: u64 = parts[3].parse().unwrap_or(0);
                        let idle: u64 = parts[4].parse().unwrap_or(0);

                        let total = user + nice + system + idle;
                        let used = user + nice + system;

                        if total > 0 {
                            (used as f64) / (total as f64)
                        } else {
                            0.0
                        }
                    } else {
                        0.0
                    }
                } else {
                    0.0
                }
            }
            Err(_) => 0.0,
        }
    }

    #[cfg(target_os = "windows")]
    async fn get_cpu_usage_windows(&self) -> f64 {
        // Use Windows performance counters or WMI
        // This is simplified - in production use winapi crate
        0.0
    }

    #[cfg(target_os = "macos")]
    async fn get_cpu_usage_macos(&self) -> f64 {
        // Use mach system calls
        // This is simplified - in production use system APIs
        0.0
    }

    // Log performance summary
    pub async fn log_performance_summary(&self) {
        let metrics = self.metrics.read().await;
        let connections = self.connection_metrics.read().await;

        let elapsed = self.start_time.elapsed();
        let current_speed = self.calculate_current_speed_mbps().await;

        info!("📊 PERFORMANCE SUMMARY");
        info!("   ⏱️  Duration: {:.1}s", elapsed.as_secs_f64());
        info!(
            "   📦 Bytes transferred: {:.1}MB",
            metrics.bytes_transferred as f64 / (1024.0 * 1024.0)
        );
        info!("   🚀 Current speed: {:.1}MB/s", current_speed);
        info!("   📈 CPU usage: {:.1}%", metrics.cpu_usage * 100.0);
        info!(
            "   💾 Memory usage: {:.1}MB",
            metrics.memory_usage as f64 / (1024.0 * 1024.0)
        );
        info!("   🗜️  Compression ratio: {:.2}", metrics.compression_ratio);
        info!("   🔗 Active connections: {}", connections.len());

        for conn in connections.iter() {
            let bytes_sent = conn.bytes_sent.load(Ordering::Relaxed);
            info!(
                "      Connection {}: {:.1}MB sent, health: {:.2}",
                conn.connection_id,
                bytes_sent as f64 / (1024.0 * 1024.0),
                conn.health_score
            );
        }
    }
}

#[derive(Debug, Default)]
pub struct PerformanceTuning {
    pub increase_chunk_size: bool,
    pub increase_batch_size: bool,
    pub reduce_compression: bool,
    pub maintain_current: bool,
    pub suggested_chunk_size: Option<usize>,
    pub suggested_connection_count: Option<usize>,
}
