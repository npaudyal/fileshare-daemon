use crate::quic::QuicIntegration;
use crate::Result;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;
use tokio::time::{sleep, interval};
use tracing::{info, error};
use uuid::Uuid;

/// Demo application showing QUIC file transfer usage
/// This demonstrates how to achieve 50-100 MBPS file transfers
pub struct QuicFileTransferDemo {
    integration: QuicIntegration,
    device_id: Uuid,
}

impl QuicFileTransferDemo {
    pub async fn new(bind_port: u16) -> Result<Self> {
        let device_id = Uuid::new_v4();
        let bind_addr: SocketAddr = format!("0.0.0.0:{}", bind_port).parse().unwrap();
        
        let integration = QuicIntegration::new(
            bind_addr,
            device_id,
            format!("QuicDemo-{}", device_id),
        ).await?;
        
        Ok(Self {
            integration,
            device_id,
        })
    }
    
    /// Start the demo server
    pub async fn start_server(&self) -> Result<()> {
        info!("🚀 Starting QUIC file transfer demo server");
        self.integration.start().await?;
        
        // Start monitoring task
        self.start_monitoring().await;
        
        info!("✅ Demo server started - ready to receive connections");
        Ok(())
    }
    
    /// Connect to another demo instance and send a file
    pub async fn send_file_demo(
        &self,
        peer_addr: SocketAddr,
        file_path: PathBuf,
    ) -> Result<()> {
        info!("📤 Demo: Sending file {:?} to {}", file_path, peer_addr);
        
        // Connect to peer
        let peer_id = Uuid::new_v4();
        self.integration.connect_to_peer(peer_addr, peer_id).await?;
        
        // Wait a bit for connection to establish
        sleep(Duration::from_millis(500)).await;
        
        // Send the file
        let transfer_id = self.integration.send_file(
            peer_id,
            file_path.clone(),
            Some("downloads".to_string()),
        ).await?;
        
        info!("🚀 File transfer started: {}", transfer_id);
        
        // Monitor progress
        self.monitor_transfer(transfer_id).await;
        
        Ok(())
    }
    
    async fn monitor_transfer(&self, transfer_id: Uuid) {
        let start_time = std::time::Instant::now();
        let mut last_bytes = 0u64;
        let mut interval = interval(Duration::from_secs(1));
        
        loop {
            interval.tick().await;
            
            if let Some(status) = self.integration.get_transfer_status(transfer_id).await {
                match status {
                    crate::quic::TransferStatus::Completed => {
                        let elapsed = start_time.elapsed();
                        info!("🎉 Transfer completed in {:.1}s", elapsed.as_secs_f64());
                        break;
                    }
                    crate::quic::TransferStatus::Failed(error) => {
                        error!("❌ Transfer failed: {}", error);
                        break;
                    }
                    crate::quic::TransferStatus::Active { streams_active } => {
                        if let Some((bytes_transferred, total_bytes, throughput)) = 
                            self.integration.get_transfer_progress(transfer_id).await {
                            
                            let progress = (bytes_transferred as f64 / total_bytes as f64) * 100.0;
                            let bytes_this_second = bytes_transferred.saturating_sub(last_bytes);
                            let throughput_mbps = throughput / (1024.0 * 1024.0);
                            let current_mbps = bytes_this_second as f64 / (1024.0 * 1024.0);
                            
                            info!("📊 Transfer Progress: {:.1}% ({}/{} MB) - {:.1} MB/s current, {:.1} MB/s avg - {} streams",
                                  progress,
                                  bytes_transferred / (1024 * 1024),
                                  total_bytes / (1024 * 1024),
                                  current_mbps,
                                  throughput_mbps,
                                  streams_active);
                            
                            last_bytes = bytes_transferred;
                        }
                    }
                    _ => {
                        info!("📝 Transfer status: {:?}", status);
                    }
                }
            } else {
                error!("❌ Transfer {} not found", transfer_id);
                break;
            }
        }
    }
    
    async fn start_monitoring(&self) {
        let integration = self.integration.clone();
        tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(5));
            
            loop {
                interval.tick().await;
                
                let stats = integration.get_connection_stats().await;
                if !stats.is_empty() {
                    info!("📈 Connection Stats:");
                    for (peer_id, stat) in stats {
                        info!("  Peer {}: {:.1} MB sent, {:.1} MB recv, RTT: {:?}, {} streams",
                              peer_id,
                              stat.bytes_sent as f64 / (1024.0 * 1024.0),
                              stat.bytes_received as f64 / (1024.0 * 1024.0),
                              stat.rtt,
                              stat.data_streams);
                    }
                }
            }
        });
    }
}

impl Clone for QuicFileTransferDemo {
    fn clone(&self) -> Self {
        Self {
            integration: self.integration.clone(),
            device_id: self.device_id,
        }
    }
}

/// Example usage functions
pub mod examples {
    use super::*;
    
    /// Run as server (receiver)
    pub async fn run_server(port: u16) -> Result<()> {
        let demo = QuicFileTransferDemo::new(port).await?;
        demo.start_server().await?;
        
        // Keep server running
        loop {
            sleep(Duration::from_secs(1)).await;
        }
    }
    
    /// Run as client (sender)
    pub async fn run_client(
        local_port: u16,
        server_addr: SocketAddr,
        file_path: PathBuf,
    ) -> Result<()> {
        let demo = QuicFileTransferDemo::new(local_port).await?;
        demo.start_server().await?;
        
        // Wait a bit for server to start
        sleep(Duration::from_secs(1)).await;
        
        // Send file
        demo.send_file_demo(server_addr, file_path).await?;
        
        // Keep running to see completion
        sleep(Duration::from_secs(30)).await;
        
        Ok(())
    }
    
    /// Benchmark function to test throughput
    pub async fn benchmark_throughput(
        local_port: u16,
        server_addr: SocketAddr,
        file_path: PathBuf,
    ) -> Result<f64> {
        let demo = QuicFileTransferDemo::new(local_port).await?;
        demo.start_server().await?;
        
        sleep(Duration::from_secs(1)).await;
        
        let start_time = std::time::Instant::now();
        let file_size = std::fs::metadata(&file_path)?.len();
        
        info!("🔥 Starting throughput benchmark for {:.1} MB file", 
              file_size as f64 / (1024.0 * 1024.0));
        
        demo.send_file_demo(server_addr, file_path).await?;
        
        let duration = start_time.elapsed();
        let throughput_mbps = (file_size as f64 / (1024.0 * 1024.0)) / duration.as_secs_f64();
        
        info!("🏆 Benchmark complete: {:.1} MB/s average throughput", throughput_mbps);
        
        Ok(throughput_mbps)
    }
}