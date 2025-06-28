use crate::{FileshareError, Result};
use crate::http::{HttpFileServer, HttpFileClient};
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

pub struct HttpTransferManager {
    http_server: Arc<HttpFileServer>,
    http_client: HttpFileClient,
    http_port: u16,
    local_ip: Option<IpAddr>,
    active_transfers: Arc<RwLock<dashmap::DashMap<Uuid, TransferRequest>>>,
}

impl HttpTransferManager {
    pub async fn new(http_port: u16) -> Result<Self> {
        let http_server = Arc::new(HttpFileServer::new(http_port));
        let http_client = HttpFileClient::new();
        
        // Try to detect local IP
        let local_ip = Self::detect_local_ip().await;
        info!("🌐 HTTP Transfer Manager initialized on port {} (IP: {:?})", http_port, local_ip);
        
        Ok(Self {
            http_server,
            http_client,
            http_port,
            local_ip,
            active_transfers: Arc::new(RwLock::new(dashmap::DashMap::new())),
        })
    }

    /// Start the HTTP server
    pub async fn start_server(&self) -> Result<()> {
        let server = self.http_server.clone();
        let server_for_spawn = server.clone();
        tokio::spawn(async move {
            if let Err(e) = server_for_spawn.start().await {
                error!("❌ HTTP server failed: {}", e);
            }
        });
        info!("✅ HTTP file server started on port {}", self.http_port);
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

        // Register file and get token
        let token = self.http_server.register_file(file_path.clone());
        
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
        info!("🔗 File ready for download at: {}", download_url);
        
        Ok(download_url)
    }

    /// Download a file from HTTP URL
    pub async fn download_file(
        &self,
        url: String,
        target_path: PathBuf,
        expected_size: Option<u64>,
    ) -> Result<()> {
        info!("⬇️ Starting HTTP download to: {:?}", target_path);
        
        // Ensure target directory exists
        if let Some(parent) = target_path.parent() {
            tokio::fs::create_dir_all(parent).await
                .map_err(|e| FileshareError::FileOperation(format!("Failed to create directory: {}", e)))?;
        }

        self.http_client.download_file(url, target_path, expected_size).await
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
        // Try to find the best local IP
        let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
        socket.connect("8.8.8.8:80").ok()?;
        let addr = socket.local_addr().ok()?;
        Some(addr.ip())
    }

    /// Detect best IP to use for a specific peer
    async fn detect_ip_for_peer(&self, peer_addr: SocketAddr) -> IpAddr {
        if peer_addr.ip().is_loopback() {
            IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
        } else if let Some(local_ip) = self.local_ip {
            local_ip
        } else {
            // Fallback to trying to detect
            Self::detect_local_ip().await
                .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST))
        }
    }

    /// Get transfer statistics
    pub async fn get_active_transfers(&self) -> Vec<TransferRequest> {
        let transfers = self.active_transfers.read().await;
        transfers.iter().map(|entry| entry.value().clone()).collect()
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