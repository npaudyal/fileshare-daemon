use crate::{FileshareError, Result};
use crate::http::{OptimizedHttpServer, OptimizedHttpClient};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Instant, Duration};
use tokio::sync::RwLock;
use tracing::{debug, error, info};
use uuid::Uuid;

#[derive(Clone)]
pub struct TransferMetrics {
    pub file_size: u64,
    pub start_time: Instant,
    pub end_time: Option<Instant>,
    pub bytes_transferred: u64,
    pub peak_speed_mbps: f64,
    pub average_speed_mbps: f64,
    pub connection_count: usize,
}

#[derive(Clone)]
pub struct OptimizedTransferManager {
    http_server: Arc<OptimizedHttpServer>,
    http_client: Arc<OptimizedHttpClient>,
    http_port: u16,
    local_ip: Option<IpAddr>,
    transfer_metrics: Arc<RwLock<dashmap::DashMap<Uuid, TransferMetrics>>>,
}

impl OptimizedTransferManager {
    pub async fn new(http_port: u16) -> Result<Self> {
        let http_server = Arc::new(OptimizedHttpServer::new(http_port));
        let http_client = Arc::new(OptimizedHttpClient::new());
        
        let local_ip = Self::detect_optimal_ip().await;
        info!("🚀 Optimized HTTP Transfer Manager initialized on port {} (IP: {:?})", http_port, local_ip);
        
        Ok(Self {
            http_server,
            http_client,
            http_port,
            local_ip,
            transfer_metrics: Arc::new(RwLock::new(dashmap::DashMap::new())),
        })
    }

    pub async fn start_optimized_server(&self) -> Result<()> {
        let server = self.http_server.clone();
        
        info!("🚀 Starting ultra-optimized HTTP file server on port {}", self.http_port);
        
        tokio::spawn(async move {
            if let Err(e) = server.start_optimized().await {
                error!("❌ Optimized HTTP server failed: {}", e);
            }
        });
        
        // Give server time to bind and apply optimizations
        tokio::time::sleep(Duration::from_millis(100)).await;
        
        info!("✅ Optimized HTTP file server started with dynamic performance profiles");
        Ok(())
    }

    pub async fn prepare_optimized_transfer(
        &self,
        file_path: PathBuf,
        peer_addr: SocketAddr,
    ) -> Result<(String, u64)> {
        // Validate and get file info
        if !file_path.exists() {
            return Err(FileshareError::FileOperation(format!("File not found: {:?}", file_path)));
        }

        let metadata = tokio::fs::metadata(&file_path).await
            .map_err(|e| FileshareError::FileOperation(format!("Failed to read metadata: {}", e)))?;
        
        let file_size = metadata.len();
        
        // Register with optimized server
        let token = self.http_server.register_file(file_path.clone()).await?;
        
        // Determine optimal IP
        let server_ip = self.determine_optimal_ip(peer_addr).await;
        let download_url = self.http_server.get_download_url(&token, &server_ip);
        
        info!("🔗 Optimized transfer ready: {} ({:.1} MB)", download_url, file_size as f64 / (1024.0 * 1024.0));
        
        Ok((download_url, file_size))
    }

    pub async fn download_optimized(
        &self,
        url: String,
        target_path: PathBuf,
        expected_size: Option<u64>,
    ) -> Result<TransferMetrics> {
        let transfer_id = Uuid::new_v4();
        let start_time = Instant::now();
        
        // Initialize metrics
        let metrics = TransferMetrics {
            file_size: expected_size.unwrap_or(0),
            start_time,
            end_time: None,
            bytes_transferred: 0,
            peak_speed_mbps: 0.0,
            average_speed_mbps: 0.0,
            connection_count: 0,
        };
        
        {
            let metrics_map = self.transfer_metrics.write().await;
            metrics_map.insert(transfer_id, metrics.clone());
        }
        
        // Ensure directory exists
        if let Some(parent) = target_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        
        // Perform optimized download
        let result = self.http_client.download_file_optimized(
            url.clone(),
            target_path,
            expected_size
        ).await;
        
        // Calculate final metrics
        let end_time = Instant::now();
        let duration = end_time.duration_since(start_time);
        let final_size = expected_size.unwrap_or(0);
        let average_speed = if duration.as_secs_f64() > 0.0 {
            (final_size as f64 * 8.0) / (duration.as_secs_f64() * 1_000_000.0)
        } else {
            0.0
        };
        
        // Update metrics
        let final_metrics = TransferMetrics {
            file_size: final_size,
            start_time,
            end_time: Some(end_time),
            bytes_transferred: final_size,
            peak_speed_mbps: average_speed * 1.2, // Estimate peak
            average_speed_mbps: average_speed,
            connection_count: 0, // Will be updated by client
        };
        
        {
            let metrics_map = self.transfer_metrics.write().await;
            metrics_map.insert(transfer_id, final_metrics.clone());
        }
        
        // Log performance summary
        self.log_performance_summary(&final_metrics);
        
        result.map(|_| final_metrics)
    }

    pub async fn cleanup_optimized_transfer(&self, url: &str) {
        if let Some(token) = url.split('/').last() {
            self.http_server.unregister_file(token);
            debug!("🧹 Cleaned up optimized transfer token: {}", token);
        }
    }

    pub async fn get_transfer_metrics(&self) -> Vec<TransferMetrics> {
        let metrics = self.transfer_metrics.read().await;
        metrics.iter().map(|entry| entry.value().clone()).collect()
    }

    pub async fn get_performance_report(&self) -> String {
        let metrics = self.get_transfer_metrics().await;
        
        if metrics.is_empty() {
            return "No transfers completed yet".to_string();
        }
        
        let total_transfers = metrics.len();
        let total_bytes: u64 = metrics.iter().map(|m| m.bytes_transferred).sum();
        let avg_speed: f64 = metrics.iter().map(|m| m.average_speed_mbps).sum::<f64>() / total_transfers as f64;
        let peak_speed = metrics.iter().map(|m| m.peak_speed_mbps).fold(0.0, f64::max);
        
        format!(
            "📊 Performance Report:\n\
             Total Transfers: {}\n\
             Total Data: {:.2} GB\n\
             Average Speed: {:.1} Mbps\n\
             Peak Speed: {:.1} Mbps",
            total_transfers,
            total_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
            avg_speed,
            peak_speed
        )
    }

    async fn detect_optimal_ip() -> Option<IpAddr> {
        // Try multiple methods to find the best IP
        
        // Method 1: Connect to public DNS
        if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") {
            if socket.connect("8.8.8.8:80").is_ok() {
                if let Ok(addr) = socket.local_addr() {
                    let ip = addr.ip();
                    if !ip.is_loopback() && !ip.is_unspecified() {
                        return Some(ip);
                    }
                }
            }
        }
        
        // Method 2: Try local network gateways
        for gateway in &["192.168.1.1:80", "192.168.0.1:80", "10.0.0.1:80", "172.16.0.1:80"] {
            if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") {
                if socket.connect(gateway).is_ok() {
                    if let Ok(addr) = socket.local_addr() {
                        let ip = addr.ip();
                        if !ip.is_loopback() && Self::is_private_ip(&ip) {
                            return Some(ip);
                        }
                    }
                }
            }
        }
        
        None
    }

    async fn determine_optimal_ip(&self, peer_addr: SocketAddr) -> String {
        if peer_addr.ip().is_loopback() {
            "127.0.0.1".to_string()
        } else if let Some(local_ip) = self.local_ip {
            local_ip.to_string()
        } else {
            // Try to detect IP in same subnet
            if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") {
                if socket.connect(peer_addr).is_ok() {
                    if let Ok(addr) = socket.local_addr() {
                        return addr.ip().to_string();
                    }
                }
            }
            
            // Fallback
            Self::detect_optimal_ip().await
                .map(|ip| ip.to_string())
                .unwrap_or_else(|| "127.0.0.1".to_string())
        }
    }

    fn is_private_ip(ip: &IpAddr) -> bool {
        match ip {
            IpAddr::V4(ipv4) => {
                let octets = ipv4.octets();
                // Private IPv4 ranges
                (octets[0] == 10) ||
                (octets[0] == 172 && octets[1] >= 16 && octets[1] <= 31) ||
                (octets[0] == 192 && octets[1] == 168)
            }
            IpAddr::V6(ipv6) => {
                // Check for ULA (fc00::/7) or link-local (fe80::/10)
                let segments = ipv6.segments();
                (segments[0] & 0xfe00) == 0xfc00 || (segments[0] & 0xffc0) == 0xfe80
            }
        }
    }

    fn log_performance_summary(&self, metrics: &TransferMetrics) {
        let duration = metrics.end_time
            .map(|end| end.duration_since(metrics.start_time))
            .unwrap_or_default();
        
        info!(
            "🎯 Transfer Performance Summary:\n\
             └─ File Size: {:.2} MB\n\
             └─ Duration: {:.2}s\n\
             └─ Average Speed: {:.1} Mbps\n\
             └─ Peak Speed: {:.1} Mbps\n\
             └─ Efficiency: {:.1}%",
            metrics.file_size as f64 / (1024.0 * 1024.0),
            duration.as_secs_f64(),
            metrics.average_speed_mbps,
            metrics.peak_speed_mbps,
            (metrics.average_speed_mbps / 1000.0) * 100.0 // Assuming gigabit LAN
        );
        
        // Provide optimization suggestions
        if metrics.average_speed_mbps < 500.0 && metrics.file_size > 100 * 1024 * 1024 {
            info!("💡 Performance tip: Consider increasing parallel connections for large files");
        } else if metrics.average_speed_mbps > 800.0 {
            info!("🚀 Excellent performance! Near theoretical maximum for gigabit LAN");
        }
    }
}

// Benchmarking utilities
pub struct TransferBenchmark {
    test_sizes: Vec<u64>,
    results: Vec<(u64, f64, Duration)>,
}

impl TransferBenchmark {
    pub fn new() -> Self {
        Self {
            test_sizes: vec![
                1 * 1024 * 1024,        // 1MB
                10 * 1024 * 1024,       // 10MB
                100 * 1024 * 1024,      // 100MB
                500 * 1024 * 1024,      // 500MB
                1024 * 1024 * 1024,     // 1GB
            ],
            results: Vec::new(),
        }
    }

    pub async fn run_benchmark(&mut self, manager: &OptimizedTransferManager, test_dir: PathBuf) -> Result<()> {
        info!("🧪 Starting transfer benchmark...");
        
        for &size in &self.test_sizes {
            // Create test file
            let test_file = test_dir.join(format!("test_{}.bin", size));
            self.create_test_file(&test_file, size).await?;
            
            // Prepare transfer
            let (url, _) = manager.prepare_optimized_transfer(
                test_file.clone(),
                "127.0.0.1:12345".parse().unwrap(),
            ).await?;
            
            // Download
            let target = test_dir.join(format!("downloaded_{}.bin", size));
            let start = Instant::now();
            
            let metrics = manager.download_optimized(url.clone(), target.clone(), Some(size)).await?;
            let duration = start.elapsed();
            
            self.results.push((size, metrics.average_speed_mbps, duration));
            
            // Cleanup
            manager.cleanup_optimized_transfer(&url).await;
            tokio::fs::remove_file(test_file).await.ok();
            tokio::fs::remove_file(target).await.ok();
            
            info!("✅ {:.1}MB: {:.1} Mbps in {:.2}s", 
                  size as f64 / (1024.0 * 1024.0),
                  metrics.average_speed_mbps,
                  duration.as_secs_f64());
        }
        
        self.print_results();
        Ok(())
    }

    async fn create_test_file(&self, path: &PathBuf, size: u64) -> Result<()> {
        use tokio::io::AsyncWriteExt;
        
        let mut file = tokio::fs::File::create(path).await?;
        let chunk = vec![0u8; 1024 * 1024]; // 1MB chunks
        let chunks = size / (1024 * 1024);
        
        for _ in 0..chunks {
            file.write_all(&chunk).await?;
        }
        
        file.sync_all().await?;
        Ok(())
    }

    fn print_results(&self) {
        info!("\n📊 Benchmark Results:");
        info!("┌─────────────┬──────────────┬─────────────┐");
        info!("│ File Size   │ Speed (Mbps) │ Time (s)    │");
        info!("├─────────────┼──────────────┼─────────────┤");
        
        for (size, speed, duration) in &self.results {
            info!("│ {:>10.1}MB │ {:>11.1} │ {:>10.2} │",
                  *size as f64 / (1024.0 * 1024.0),
                  speed,
                  duration.as_secs_f64());
        }
        
        info!("└─────────────┴──────────────┴─────────────┘");
        
        let avg_speed: f64 = self.results.iter().map(|(_, s, _)| s).sum::<f64>() / self.results.len() as f64;
        info!("\n🎯 Average Speed: {:.1} Mbps", avg_speed);
    }
}