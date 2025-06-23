use crate::quic::{
    connection::QuicConnectionManager,
    transfer::QuicTransferManager,
    stream_manager::StreamManager,
    protocol::*,
};
use crate::Result;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock, Mutex};
use tracing::{info, error, warn, debug};
use uuid::Uuid;

/// Integration layer between QUIC file transfer and existing daemon
#[derive(Clone)]
pub struct QuicIntegration {
    connection_manager: Arc<QuicConnectionManager>,
    transfer_manager: Arc<QuicTransferManager>,
    stream_manager: Arc<StreamManager>,
    message_processor: Arc<QuicMessageProcessor>,
    device_id: Uuid,
    bind_addr: SocketAddr,
}

impl QuicIntegration {
    pub async fn new(
        bind_addr: SocketAddr,
        device_id: Uuid,
        device_name: String,
    ) -> Result<Self> {
        // Create stream manager
        let stream_manager = Arc::new(StreamManager::new());
        
        // Create connection manager
        let connection_manager = Arc::new(
            QuicConnectionManager::new(bind_addr, device_id, device_name).await?
        );
        
        // Create transfer manager
        let transfer_manager = Arc::new(
            QuicTransferManager::new(stream_manager.clone(), 4) // Max 4 concurrent transfers
        );
        
        // Create message processor
        let message_processor = Arc::new(QuicMessageProcessor::new(
            connection_manager.clone(),
            transfer_manager.clone(),
            stream_manager.clone(),
        ));
        
        Ok(Self {
            connection_manager,
            transfer_manager,
            stream_manager,
            message_processor,
            device_id,
            bind_addr,
        })
    }
    
    /// Start the QUIC integration
    pub async fn start(&self) -> Result<()> {
        info!("🚀 Starting QUIC file transfer integration on {}", self.bind_addr);
        
        // Start connection manager
        self.connection_manager.start().await?;
        
        // Start message processor
        self.message_processor.start().await?;
        
        info!("✅ QUIC integration started successfully");
        Ok(())
    }
    
    /// Connect to a peer
    pub async fn connect_to_peer(&self, peer_addr: SocketAddr, peer_id: Uuid) -> Result<()> {
        self.connection_manager.connect_to_peer(peer_addr, peer_id).await
    }
    
    /// Send a file to a peer
    /// Request a file from a peer via QUIC
    pub async fn request_file(
        &self,
        peer_id: Uuid,
        file_path: String,
        target_path: String,
    ) -> Result<Uuid> {
        let request_id = Uuid::new_v4();
        let message = QuicMessage::FileRequest {
            request_id,
            file_path,
            target_path,
        };
        
        self.connection_manager.send_message(peer_id, message).await?;
        info!("📤 Sent QUIC file request {} to peer {}", request_id, peer_id);
        Ok(request_id)
    }

    /// Send a file to a peer via QUIC (for senders)
    pub async fn send_file(
        &self,
        peer_id: Uuid,
        file_path: PathBuf,
        target_dir: Option<String>,
    ) -> Result<Uuid> {
        let stream_count = 8; // Use 8 parallel streams for maximum throughput
        self.transfer_manager.send_file(peer_id, file_path, target_dir, stream_count).await
    }
    
    /// Get transfer status
    pub async fn get_transfer_status(&self, transfer_id: Uuid) -> Option<crate::quic::transfer::TransferStatus> {
        self.transfer_manager.get_transfer_status(transfer_id).await
    }
    
    /// Get transfer progress
    pub async fn get_transfer_progress(&self, transfer_id: Uuid) -> Option<(u64, u64, f64)> {
        self.transfer_manager.get_transfer_progress(transfer_id).await
    }
    
    /// Get connection statistics
    pub async fn get_connection_stats(&self) -> HashMap<Uuid, crate::quic::connection::ConnectionStats> {
        self.connection_manager.get_connection_stats().await
    }
}

/// Processes QUIC messages and coordinates file transfers
pub struct QuicMessageProcessor {
    connection_manager: Arc<QuicConnectionManager>,
    transfer_manager: Arc<QuicTransferManager>,
    stream_manager: Arc<StreamManager>,
    pending_offers: Arc<RwLock<HashMap<Uuid, QuicFileMetadata>>>,
}

impl QuicMessageProcessor {
    pub fn new(
        connection_manager: Arc<QuicConnectionManager>,
        transfer_manager: Arc<QuicTransferManager>,
        stream_manager: Arc<StreamManager>,
    ) -> Self {
        Self {
            connection_manager,
            transfer_manager,
            stream_manager,
            pending_offers: Arc::new(RwLock::new(HashMap::new())),
        }
    }
    
    pub async fn start(&self) -> Result<()> {
        let message_receiver = self.connection_manager.get_message_receiver();
        let processor = self.clone();
        
        tokio::spawn(async move {
            let mut receiver = message_receiver.lock().await;
            
            while let Some((peer_id, message)) = receiver.recv().await {
                if let Err(e) = processor.process_message(peer_id, message).await {
                    error!("❌ Failed to process message from {}: {}", peer_id, e);
                }
            }
        });
        
        info!("✅ QUIC message processor started");
        Ok(())
    }
    
    async fn process_message(&self, peer_id: Uuid, message: QuicMessage) -> Result<()> {
        debug!("📨 Processing message from {}: {:?}", peer_id, message);
        
        match message {
            QuicMessage::Handshake { device_id, device_name, version, capabilities } => {
                self.handle_handshake(peer_id, device_id, device_name, version, capabilities).await
            }
            
            QuicMessage::HandshakeResponse { accepted, capabilities, reason } => {
                self.handle_handshake_response(peer_id, accepted, capabilities, reason).await
            }
            
            QuicMessage::FileRequest { request_id, file_path, target_path } => {
                self.handle_file_request(peer_id, request_id, file_path, target_path).await
            }
            
            QuicMessage::FileOffer { transfer_id, metadata, stream_count } => {
                self.handle_file_offer(peer_id, transfer_id, metadata, stream_count).await
            }
            
            QuicMessage::FileOfferResponse { transfer_id, accepted, reason } => {
                self.handle_file_offer_response(peer_id, transfer_id, accepted, reason).await
            }
            
            QuicMessage::TransferStart { transfer_id, stream_assignments } => {
                self.handle_transfer_start(peer_id, transfer_id, stream_assignments).await
            }
            
            QuicMessage::TransferComplete { transfer_id, checksum, total_bytes } => {
                self.handle_transfer_complete(peer_id, transfer_id, checksum, total_bytes).await
            }
            
            QuicMessage::TransferError { transfer_id, error_type, message } => {
                self.handle_transfer_error(peer_id, transfer_id, error_type, message).await
            }
            
            _ => {
                debug!("📝 Unhandled message type from {}", peer_id);
                Ok(())
            }
        }
    }
    
    async fn handle_handshake(
        &self,
        temp_peer_id: Uuid,
        device_id: Uuid,
        device_name: String,
        version: String,
        capabilities: TransferCapabilities,
    ) -> Result<()> {
        info!("🤝 Handshake from {} ({}): {}", device_name, device_id, version);
        
        // CRITICAL: Update the connection mapping to use real device ID
        // This fixes the connection ID mismatch issue
        if let Err(e) = self.connection_manager.update_peer_id(temp_peer_id, device_id).await {
            warn!("⚠️ Failed to update peer ID mapping: {}", e);
        } else {
            info!("✅ Updated QUIC connection mapping: {} -> {}", temp_peer_id, device_id);
        }
        
        // Send handshake response using the real device ID
        let response = QuicMessage::HandshakeResponse {
            accepted: true,
            capabilities: TransferCapabilities::default(),
            reason: None,
        };
        
        // Try to send with real device ID first, fall back to temp ID if needed
        let send_result = self.connection_manager.send_message(device_id, response.clone()).await;
        if send_result.is_err() {
            warn!("⚠️ Sending with real device ID failed, using temp ID");
            self.connection_manager.send_message(temp_peer_id, response).await?;
        }
        
        info!("✅ Handshake completed with {} (device_id: {})", device_name, device_id);
        Ok(())
    }
    
    async fn handle_handshake_response(
        &self,
        peer_id: Uuid,
        accepted: bool,
        capabilities: TransferCapabilities,
        reason: Option<String>,
    ) -> Result<()> {
        if accepted {
            info!("🤝 Handshake accepted by peer {}: {:?}", peer_id, capabilities);
            info!("✅ QUIC peer {} is ready for file transfers", peer_id);
        } else {
            warn!("❌ Handshake rejected by peer {}: {:?}", peer_id, reason);
        }
        Ok(())
    }
    
    async fn handle_file_request(
        &self,
        peer_id: Uuid,
        request_id: Uuid,
        file_path: String,
        target_path: String,
    ) -> Result<()> {
        info!("📥 Received file request {} from peer {}: {} -> {}", 
              request_id, peer_id, file_path, target_path);
        
        // Validate file exists and is accessible
        let source_path = std::path::PathBuf::from(&file_path);
        if !source_path.exists() {
            warn!("❌ Requested file does not exist: {}", file_path);
            return Err(crate::FileshareError::FileOperation(format!("File not found: {}", file_path)));
        }
        
        // Start file transfer
        match self.transfer_manager.send_file(peer_id, source_path, None, 8).await {
            Ok(transfer_id) => {
                info!("✅ Started QUIC file transfer {} for request {}", transfer_id, request_id);
                Ok(())
            }
            Err(e) => {
                error!("❌ Failed to start file transfer for request {}: {}", request_id, e);
                Err(e)
            }
        }
    }
    
    async fn handle_file_offer(
        &self,
        peer_id: Uuid,
        transfer_id: Uuid,
        metadata: QuicFileMetadata,
        stream_count: u8,
    ) -> Result<()> {
        info!("📄 File offer: {} ({:.1} MB) from {} using {} streams",
              metadata.name, metadata.size as f64 / (1024.0 * 1024.0), peer_id, stream_count);
        
        // Store pending offer
        {
            let mut offers = self.pending_offers.write().await;
            offers.insert(transfer_id, metadata.clone());
        }
        
        // For now, auto-accept all offers (in real app, this would show UI prompt)
        let response = QuicMessage::FileOfferResponse {
            transfer_id,
            accepted: true,
            reason: None,
        };
        
        self.connection_manager.send_message(peer_id, response).await?;
        
        // Start receiving the file
        let save_path = PathBuf::from(format!("./downloads/{}", metadata.name));
        if let Some(parent) = save_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        
        self.transfer_manager.receive_file(transfer_id, peer_id, metadata, save_path).await?;
        
        Ok(())
    }
    
    async fn handle_file_offer_response(
        &self,
        peer_id: Uuid,
        transfer_id: Uuid,
        accepted: bool,
        reason: Option<String>,
    ) -> Result<()> {
        if accepted {
            info!("✅ File offer {} accepted by {}", transfer_id, peer_id);
            
            // Open data streams for the transfer
            self.stream_manager.open_data_streams(peer_id, 8).await?;
            
            // Send transfer start message
            let mut stream_assignments = HashMap::new();
            for i in 1..=8u8 {
                stream_assignments.insert(i, (i - 1) as u64); // Simple assignment for now
            }
            
            let start_message = QuicMessage::TransferStart {
                transfer_id,
                stream_assignments,
            };
            
            self.connection_manager.send_message(peer_id, start_message).await?;
        } else {
            warn!("❌ File offer {} rejected by {}: {:?}", transfer_id, peer_id, reason);
        }
        
        Ok(())
    }
    
    async fn handle_transfer_start(
        &self,
        peer_id: Uuid,
        transfer_id: Uuid,
        stream_assignments: HashMap<u8, u64>,
    ) -> Result<()> {
        info!("🚀 Transfer {} starting with {} streams", transfer_id, stream_assignments.len());
        
        // Start chunk receivers for the transfer
        self.stream_manager.start_chunk_receivers(peer_id, transfer_id).await?;
        
        Ok(())
    }
    
    async fn handle_transfer_complete(
        &self,
        peer_id: Uuid,
        transfer_id: Uuid,
        checksum: String,
        total_bytes: u64,
    ) -> Result<()> {
        info!("🎉 Transfer {} completed: {:.1} MB, checksum: {}",
              transfer_id, total_bytes as f64 / (1024.0 * 1024.0), checksum);
        
        // Clean up resources
        self.stream_manager.close_peer_streams(peer_id).await?;
        
        Ok(())
    }
    
    async fn handle_transfer_error(
        &self,
        peer_id: Uuid,
        transfer_id: Uuid,
        error_type: TransferError,
        message: String,
    ) -> Result<()> {
        error!("❌ Transfer {} error from {}: {:?} - {}", transfer_id, peer_id, error_type, message);
        
        // Clean up resources
        self.stream_manager.close_peer_streams(peer_id).await.ok();
        
        Ok(())
    }
}

impl Clone for QuicMessageProcessor {
    fn clone(&self) -> Self {
        Self {
            connection_manager: self.connection_manager.clone(),
            transfer_manager: self.transfer_manager.clone(),
            stream_manager: self.stream_manager.clone(),
            pending_offers: self.pending_offers.clone(),
        }
    }
}