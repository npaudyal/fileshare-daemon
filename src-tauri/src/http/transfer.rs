use crate::{FileshareError, Result};
use crate::http::{OptimizedHttpServer, OptimizedHttpClient};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info};
use uuid::Uuid;

#[derive(Clone)]
pub struct TransferRequest {
    pub request_id: Uuid,
    pub file_path: PathBuf,
    pub target_path: PathBuf,
    pub file_size: u64,
    pub peer_id: Uuid,
}

#[derive(Clone)]
pub struct TransferStats {
    pub total_bytes: u64,
    pub transferred_bytes: u64,
    pub start_time: std::time::Instant,
    pub speed_mbps: f64,
}

pub struct HttpTransferManager {
    http_server: Arc<OptimizedHttpServer>,
    http_client: OptimizedHttpClient,
    http_port: u16,
    local_ip: Option<IpAddr>,
    active_transfers: Arc<RwLock<dashmap::DashMap<Uuid, TransferStats>>>,
}

impl HttpTransferManager {
    pub async fn new(http_port: u16) -> Result<Self> {
        let http_server = Arc::new(OptimizedHttpServer::new(http_port));
        let http_client = OptimizedHttpClient::new();
        
        // Try to detect local IP
        let local_ip = Self::detect_local_ip().await;
        info!("🚀 OPTIMIZED HTTP Transfer Manager initialized on port {} with ultra-fast profiles (IP: {:?})", http_port, local_ip);
        
        Ok(Self {
            http_server,
            http_client,
            http_port,
            local_ip,
            active_transfers: Arc::new(RwLock::new(dashmap::DashMap::new())),
        })
    }

    /// Start the HTTP server with optimizations
    pub async fn start_server(&self) -> Result<()> {
        let server = self.http_server.clone();
        let server_for_spawn = server.clone();
        
        // Configure server with TCP optimizations
        info!("🚀 Starting optimized HTTP file server on port {}", self.http_port);
        
        tokio::spawn(async move {
            if let Err(e) = server_for_spawn.start_optimized().await {
                error!("❌ HTTP server failed: {}", e);
            }
        });
        
        // Give server time to bind
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        
        info!("✅ HTTP file server started with performance optimizations");
        Ok(())
    }

    /// Prepare a file for transfer and return the download URL
    pub async fn prepare_file_transfer(
        &self,
        file_path: PathBuf,
        peer_addr: SocketAddr,
    ) -> Result<String> {
        // Validate file exists
        if !file_path.exists() {
            return Err(FileshareError::FileOperation(format!("File not found: {:?}", file_path)));
        }

        let metadata = tokio::fs::metadata(&file_path).await
            .map_err(|e| FileshareError::FileOperation(format!("Failed to read metadata: {}", e)))?;
        
        let file_size = metadata.len();
        info!("📦 Preparing file for HTTP transfer: {:?} ({:.1} MB)", 
              file_path, file_size as f64 / (1024.0 * 1024.0));

        // Log performance hints
        if file_size > 50 * 1024 * 1024 {
            info!("💡 Large file detected - will use parallel downloads for optimal speed");
        }

        // Register file and get token
        let token = self.http_server.register_file(file_path.clone()).await?;
        
        // Determine the best IP to use
        let server_ip = if peer_addr.ip().is_loopback() {
            "127.0.0.1".to_string()
        } else if let Some(local_ip) = self.local_ip {
            local_ip.to_string()
        } else {
            // Fallback: try to detect from peer connection
            self.detect_ip_for_peer(peer_addr).await.to_string()
        };

        let download_url = self.http_server.get_download_url(&token, &server_ip);
        info!("🔗 File ready for optimized download at: {}", download_url);
        
        Ok(download_url)
    }

    /// Download a file from HTTP URL with optimizations
    pub async fn download_file(
        &self,
        url: String,
        target_path: PathBuf,
        expected_size: Option<u64>,
    ) -> Result<()> {
        info!("⬇️ Starting optimized HTTP download to: {:?}", target_path);
        
        // Create transfer tracking
        let transfer_id = Uuid::new_v4();
        let stats = TransferStats {
            total_bytes: expected_size.unwrap_or(0),
            transferred_bytes: 0,
            start_time: std::time::Instant::now(),
            speed_mbps: 0.0,
        };
        
        {
            let transfers = self.active_transfers.write().await;
            transfers.insert(transfer_id, stats);
        }
        
        // Ensure target directory exists
        if let Some(parent) = target_path.parent() {
            tokio::fs::create_dir_all(parent).await
                .map_err(|e| FileshareError::FileOperation(format!("Failed to create directory: {}", e)))?;
        }

        // Use optimized client with dynamic profiles for maximum speed
        let result = self.http_client.download_file_optimized(url.clone(), target_path, expected_size).await;
        
        // Remove from active transfers
        {
            let transfers = self.active_transfers.write().await;
            transfers.remove(&transfer_id);
        }
        
        result
    }

    /// Cleanup after transfer
    pub async fn cleanup_transfer(&self, url: &str) {
        // Extract token from URL
        if let Some(token) = url.split('/').last() {
            self.http_server.unregister_file(token);
            debug!("🧹 Cleaned up transfer token: {}", token);
        }
    }

    /// Detect local IP address
    async fn detect_local_ip() -> Option<IpAddr> {
        // Try multiple methods to find the best local IP
        
        // Method 1: Try to connect to a public DNS server (doesn't actually connect)
        if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") {
            if socket.connect("8.8.8.8:80").is_ok() {
                if let Ok(addr) = socket.local_addr() {
                    let ip = addr.ip();
                    // Verify it's not loopback
                    if !ip.is_loopback() {
                        return Some(ip);
                    }
                }
            }
        }
        
        // Method 2: Try common LAN gateway addresses
        for gateway in &["192.168.1.1:80", "192.168.0.1:80", "10.0.0.1:80"] {
            if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") {
                if socket.connect(gateway).is_ok() {
                    if let Ok(addr) = socket.local_addr() {
                        let ip = addr.ip();
                        if !ip.is_loopback() && (ip.is_ipv4() && ip.to_string().starts_with("192.168.") || 
                                                  ip.to_string().starts_with("10.") || 
                                                  ip.to_string().starts_with("172.")) {
                            return Some(ip);
                        }
                    }
                }
            }
        }
        
        None
    }

    /// Detect best IP to use for a specific peer
    async fn detect_ip_for_peer(&self, peer_addr: SocketAddr) -> IpAddr {
        if peer_addr.ip().is_loopback() {
            IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
        } else if let Some(local_ip) = self.local_ip {
            local_ip
        } else {
            // Try to detect IP in the same subnet as peer
            if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") {
                if socket.connect(peer_addr).is_ok() {
                    if let Ok(addr) = socket.local_addr() {
                        return addr.ip();
                    }
                }
            }
            
            // Fallback
            Self::detect_local_ip().await
                .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST))
        }
    }

    /// Get transfer statistics
    pub async fn get_active_transfers(&self) -> Vec<(Uuid, TransferStats)> {
        let transfers = self.active_transfers.read().await;
        transfers.iter().map(|entry| (*entry.key(), entry.value().clone())).collect()
    }

    /// Get current transfer speed for all active transfers
    pub async fn get_total_speed_mbps(&self) -> f64 {
        let transfers = self.active_transfers.read().await;
        transfers.iter()
            .map(|entry| entry.value().speed_mbps)
            .sum()
    }
}

impl Clone for HttpTransferManager {
    fn clone(&self) -> Self {
        Self {
            http_server: self.http_server.clone(),
            http_client: self.http_client.clone(),
            http_port: self.http_port,
            local_ip: self.local_ip,
            active_transfers: self.active_transfers.clone(),
        }
    }
}