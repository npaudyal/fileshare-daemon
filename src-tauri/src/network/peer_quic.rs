use crate::{
    config::Settings,
    network::{discovery::DeviceInfo, protocol::*},
    quic::{
        ParallelTransferManager, QuicConnection, QuicConnectionManager, QuicFileTransfer,
        StreamManager,
    },
    service::file_transfer::{
        FileTransferManager, MessageSender, TransferDirection, TransferStatus,
    },
    FileshareError, Result,
};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, RwLock};
use tokio::time::{interval, timeout};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

// Constants for connection health monitoring
const PING_INTERVAL_SECONDS: u64 = 30;
const PING_TIMEOUT_SECONDS: u64 = 10;
const MAX_MISSED_PINGS: u32 = 3;
const RECONNECTION_DELAY_SECONDS: u64 = 5;
const MAX_RECONNECTION_ATTEMPTS: u32 = 5;

#[derive(Debug, Clone)]
pub struct Peer {
    pub device_info: DeviceInfo,
    pub connection_status: ConnectionStatus,
    pub last_ping: Option<Instant>,
    pub ping_failures: u32,
    pub last_seen: Instant,
    pub reconnection_attempts: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
    Authenticated,
    Reconnecting,
    Error(String),
}

#[derive(Debug, Default)]
pub struct ConnectionStats {
    pub total: usize,
    pub authenticated: usize,
    pub connected: usize,
    pub connecting: usize,
    pub reconnecting: usize,
    pub disconnected: usize,
    pub error: usize,
    pub unhealthy: usize,
}

pub struct PeerManager {
    settings: Arc<Settings>,
    pub peers: HashMap<Uuid, Peer>,
    pub file_transfer: Arc<RwLock<FileTransferManager>>,
    pub message_tx: mpsc::UnboundedSender<(Uuid, Message)>,
    pub message_rx: mpsc::UnboundedReceiver<(Uuid, Message)>,
    // QUIC components
    quic_manager: Arc<QuicConnectionManager>,
    stream_managers: HashMap<Uuid, Arc<StreamManager>>,
    parallel_transfer_manager: Arc<ParallelTransferManager>,
}

impl PeerManager {
    pub async fn new(settings: Arc<Settings>) -> Result<Self> {
        let (message_tx, message_rx) = mpsc::unbounded_channel();
        let file_transfer = Arc::new(RwLock::new(
            FileTransferManager::new(settings.clone()).await?,
        ));

        // Create QUIC connection manager
        let bind_addr: SocketAddr = format!("0.0.0.0:{}", settings.network.port)
            .parse()
            .map_err(|e| FileshareError::Network(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("Invalid address: {}", e)
            )))?;
        let quic_manager = Arc::new(QuicConnectionManager::new(bind_addr).await?);

        // Create parallel transfer manager
        let parallel_transfer_manager = Arc::new(ParallelTransferManager::new(16));

        let peer_manager = Self {
            settings,
            peers: HashMap::new(),
            file_transfer,
            message_tx: message_tx.clone(),
            message_rx,
            quic_manager,
            stream_managers: HashMap::new(),
            parallel_transfer_manager,
        };

        // Set up the message sender for file transfers
        peer_manager.set_file_transfer_message_sender().await;

        // Start accepting QUIC connections
        peer_manager.start_quic_listener();

        Ok(peer_manager)
    }

    fn start_quic_listener(&self) {
        let quic_manager = self.quic_manager.clone();
        let message_tx = self.message_tx.clone();

        tokio::spawn(async move {
            loop {
                match quic_manager.accept_connection().await {
                    Ok(connection) => {
                        info!(
                            "Accepted QUIC connection from {}",
                            connection.remote_address()
                        );

                        // Handle the connection in a separate task
                        let quic_manager = quic_manager.clone();
                        let message_tx = message_tx.clone();

                        tokio::spawn(async move {
                            if let Err(e) = Self::handle_incoming_quic_connection(
                                connection,
                                quic_manager,
                                message_tx,
                            )
                            .await
                            {
                                error!("Error handling QUIC connection: {}", e);
                            }
                        });
                    }
                    Err(e) => {
                        error!("Error accepting QUIC connection: {}", e);
                        // Brief pause before retrying
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        });
    }

    async fn handle_incoming_quic_connection(
        connection: QuicConnection,
        _quic_manager: Arc<QuicConnectionManager>,
        message_tx: mpsc::UnboundedSender<(Uuid, Message)>,
    ) -> Result<()> {
        info!("🔗 Setting up stream manager for incoming QUIC connection");
        
        // Create stream manager for this connection
        let stream_manager = Arc::new(StreamManager::new(connection.clone(), message_tx.clone()));

        // Start stream listener - this will handle incoming messages and forward them to message_tx
        stream_manager.clone().start_stream_listener().await;

        info!("✅ Stream manager started for incoming connection, handshake will be handled by main message handler");
        
        // Keep the connection alive by monitoring connection status
        // The stream manager handles all message processing in background tasks
        loop {
            if connection.is_closed() {
                info!("📴 Incoming QUIC connection closed");
                break;
            }
            
            // Sleep for a bit to avoid busy waiting
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        
        Ok(())
    }

    async fn set_file_transfer_message_sender(&self) {
        let mut ft = self.file_transfer.write().await;
        ft.set_message_sender(self.message_tx.clone());
    }

    pub async fn get_all_discovered_devices(&self) -> Vec<DeviceInfo> {
        let mut discovered = Vec::new();

        for (_, peer) in &self.peers {
            discovered.push(DeviceInfo {
                id: peer.device_info.id,
                name: peer.device_info.name.clone(),
                addr: peer.device_info.addr,
                last_seen: peer.device_info.last_seen,
                version: peer.device_info.version.clone(),
            });
        }

        discovered
    }

    pub async fn on_device_discovered(&mut self, device_info: DeviceInfo) -> Result<()> {
        // Check if peer already exists
        if let Some(existing_peer) = self.peers.get_mut(&device_info.id) {
            existing_peer.device_info = device_info.clone();
            existing_peer.last_seen = Instant::now();
            info!(
                "🔄 Updated existing peer: {} - Current status: {:?}",
                existing_peer.device_info.name, existing_peer.connection_status
            );
            
            // Check if we need to attempt connection for existing peer
            if matches!(existing_peer.connection_status, ConnectionStatus::Disconnected | ConnectionStatus::Error(_)) {
                if self.should_connect_to_peer(&device_info) {
                    info!("🔗 Attempting QUIC connection to existing peer: {}", device_info.name);
                    if let Err(e) = self.connect_to_peer(device_info.id).await {
                        error!("❌ Failed to connect to existing peer {}: {}", device_info.name, e);
                    }
                } else {
                    info!("⏭️ Skipping connection to existing peer {} (require_pairing: {})", 
                          device_info.name, self.settings.security.require_pairing);
                }
            } else {
                info!("ℹ️ Existing peer {} already has status {:?}, not attempting connection", 
                      device_info.name, existing_peer.connection_status);
            }
            return Ok(());
        }

        // Create new peer
        let peer = Peer {
            device_info: device_info.clone(),
            connection_status: ConnectionStatus::Disconnected,
            last_ping: None,
            ping_failures: 0,
            last_seen: Instant::now(),
            reconnection_attempts: 0,
        };

        info!("✅ Adding new peer: {} ({}) at {}", device_info.name, device_info.id, device_info.addr);
        self.peers.insert(device_info.id, peer);

        // Attempt to connect
        if self.should_connect_to_peer(&device_info) {
            info!("🔗 Attempting QUIC connection to peer: {}", device_info.name);
            if let Err(e) = self.connect_to_peer(device_info.id).await {
                error!("❌ Failed to connect to peer {}: {}", device_info.name, e);
            }
        } else {
            info!("⏭️ Skipping connection to peer {} (pairing required: {})", 
                  device_info.name, self.settings.security.require_pairing);
        }

        Ok(())
    }

    fn should_connect_to_peer(&self, device_info: &DeviceInfo) -> bool {
        if self.settings.security.require_pairing {
            self.settings
                .security
                .allowed_devices
                .contains(&device_info.id)
        } else {
            true
        }
    }

    pub async fn connect_to_peer(&mut self, peer_id: Uuid) -> Result<()> {
        let peer = self
            .peers
            .get_mut(&peer_id)
            .ok_or_else(|| FileshareError::Unknown("Peer not found".to_string()))?;

        if matches!(
            peer.connection_status,
            ConnectionStatus::Connected | ConnectionStatus::Authenticated
        ) {
            debug!("Peer {} already connected/authenticated", peer_id);
            return Ok(());
        }

        peer.connection_status = ConnectionStatus::Connecting;
        let addr = peer.device_info.addr;

        info!("🚀 Connecting to peer {} at {} via QUIC", peer_id, addr);

        match self.quic_manager.connect_to_peer(addr, peer_id).await {
            Ok(connection) => {
                info!("✅ Successfully established QUIC connection to peer {} at {}", peer_id, addr);
                peer.connection_status = ConnectionStatus::Connected;
                peer.last_seen = Instant::now();

                // Create stream manager
                let stream_manager = Arc::new(StreamManager::new(connection.clone(), self.message_tx.clone()));

                // Store stream manager
                self.stream_managers.insert(peer_id, stream_manager.clone());

                // Start stream listener
                stream_manager.clone().start_stream_listener().await;

                // Send handshake
                let handshake =
                    Message::handshake(self.settings.device.id, self.settings.device.name.clone());

                stream_manager.send_control_message(handshake).await?;
                
                info!("✅ Handshake sent to peer {}, waiting for response via main message handler", peer_id);
                // The handshake response will be handled by the main message handler in daemon_quic.rs
                // which will call handle_message with the HandshakeResponse
            }
            Err(e) => {
                error!("❌ Failed to establish QUIC connection to peer {}: {}", peer_id, e);
                peer.connection_status = ConnectionStatus::Error(e.to_string());
                peer.reconnection_attempts += 1;
                
                // Don't return error immediately - let discovery continue working
                warn!("⚠️ Will retry connection to peer {} later", peer_id);
            }
        }

        Ok(())
    }

    pub async fn send_message_to_peer(&mut self, peer_id: Uuid, message: Message) -> Result<()> {
        info!(
            "Attempting to send message to peer {}: {:?}",
            peer_id, message.message_type
        );

        if let Some(stream_manager) = self.stream_managers.get(&peer_id) {
            stream_manager.send_control_message(message).await?;
            info!("✅ Successfully sent message to peer {}", peer_id);
            Ok(())
        } else {
            error!("❌ No active QUIC connection to peer {}", peer_id);
            Err(FileshareError::Transfer(format!(
                "No active QUIC connection to peer {}",
                peer_id
            )))
        }
    }

    pub async fn send_direct_to_connection(&self, peer_id: Uuid, message: Message) -> Result<()> {
        if let Some(stream_manager) = self.stream_managers.get(&peer_id) {
            stream_manager.send_control_message(message).await?;
            Ok(())
        } else {
            Err(FileshareError::Transfer(format!(
                "No connection to peer {}",
                peer_id
            )))
        }
    }

    // Placeholder for TCP compatibility - QUIC connections are handled differently
    pub async fn handle_connection(&mut self, _stream: tokio::net::TcpStream) -> Result<()> {
        warn!("TCP handle_connection called on QUIC PeerManager - ignoring");
        Ok(())
    }

    pub async fn send_file_to_peer(&mut self, peer_id: Uuid, file_path: PathBuf) -> Result<()> {
        // Check if peer is connected
        let peer = self
            .peers
            .get(&peer_id)
            .ok_or_else(|| FileshareError::Unknown("Peer not found".to_string()))?;

        if !matches!(peer.connection_status, ConnectionStatus::Authenticated) {
            return Err(FileshareError::Transfer(
                "Peer not authenticated".to_string(),
            ));
        }

        // Get stream manager for this peer
        let stream_manager = self
            .stream_managers
            .get(&peer_id)
            .ok_or_else(|| FileshareError::Transfer("No stream manager for peer".to_string()))?
            .clone();

        // Start parallel file transfer using QUIC
        let transfer_id = self
            .parallel_transfer_manager
            .start_transfer(stream_manager, peer_id, file_path)
            .await?;

        info!(
            "Started QUIC file transfer {} to peer {}",
            transfer_id, peer_id
        );

        Ok(())
    }

    pub fn get_connected_peers(&self) -> Vec<Peer> {
        self.peers
            .values()
            .filter(|peer| matches!(peer.connection_status, ConnectionStatus::Authenticated))
            .cloned()
            .collect()
    }

    pub async fn check_peer_health_all(&mut self) -> Result<()> {
        let authenticated_peers: Vec<Uuid> = self
            .peers
            .iter()
            .filter(|(_, peer)| matches!(peer.connection_status, ConnectionStatus::Authenticated))
            .map(|(id, _)| *id)
            .collect();

        for peer_id in authenticated_peers {
            if let Err(e) = self.check_peer_health(peer_id).await {
                warn!("Health check failed for peer {}: {}", peer_id, e);
            }
        }

        Ok(())
    }

    async fn check_peer_health(&mut self, peer_id: Uuid) -> Result<()> {
        info!("🩺 Checking health of peer {}", peer_id);

        match self.ping_peer_with_timeout(peer_id).await {
            Ok(response_time) => {
                info!(
                    "✅ Peer {} responded to ping in {:?}",
                    peer_id, response_time
                );

                if let Some(peer) = self.peers.get_mut(&peer_id) {
                    peer.ping_failures = 0;
                    peer.last_ping = Some(Instant::now());
                    peer.last_seen = Instant::now();
                }
            }
            Err(_) => {
                warn!("❌ Peer {} failed to respond to ping", peer_id);
                self.handle_ping_failure(peer_id).await?;
            }
        }

        Ok(())
    }

    async fn ping_peer_with_timeout(&mut self, peer_id: Uuid) -> Result<Duration> {
        let start_time = Instant::now();
        let ping_message = Message::ping();

        self.send_message_to_peer(peer_id, ping_message).await?;

        // In a real implementation, wait for pong response
        // For now, simulate with a short delay
        tokio::time::sleep(Duration::from_millis(100)).await;

        Ok(start_time.elapsed())
    }

    async fn handle_ping_failure(&mut self, peer_id: Uuid) -> Result<()> {
        if let Some(peer) = self.peers.get_mut(&peer_id) {
            peer.ping_failures += 1;

            if peer.ping_failures >= MAX_MISSED_PINGS {
                warn!(
                    "💔 Peer {} exceeded max ping failures, marking as disconnected",
                    peer_id
                );
                peer.connection_status = ConnectionStatus::Disconnected;

                // Remove stream manager
                self.stream_managers.remove(&peer_id);

                // Remove from QUIC connections
                self.quic_manager.remove_connection(peer_id).await;

                // Attempt reconnection
                self.attempt_reconnection(peer_id).await?;
            }
        }

        Ok(())
    }

    async fn attempt_reconnection(&mut self, peer_id: Uuid) -> Result<()> {
        let should_reconnect = if let Some(peer) = self.peers.get_mut(&peer_id) {
            if peer.reconnection_attempts >= MAX_RECONNECTION_ATTEMPTS {
                error!("💥 Max reconnection attempts reached for peer {}", peer_id);
                peer.connection_status =
                    ConnectionStatus::Error("Max reconnection attempts exceeded".to_string());
                return Ok(());
            }

            peer.reconnection_attempts += 1;
            peer.connection_status = ConnectionStatus::Reconnecting;
            true
        } else {
            false
        };

        if should_reconnect {
            tokio::time::sleep(Duration::from_secs(RECONNECTION_DELAY_SECONDS)).await;

            match self.connect_to_peer(peer_id).await {
                Ok(_) => {
                    info!("✅ Successfully reconnected to peer {}", peer_id);
                    if let Some(peer) = self.peers.get_mut(&peer_id) {
                        peer.reconnection_attempts = 0;
                    }
                }
                Err(e) => {
                    warn!("❌ Reconnection failed for peer {}: {}", peer_id, e);
                }
            }
        }

        Ok(())
    }

    pub fn is_peer_healthy(&self, peer_id: Uuid) -> bool {
        if let Some(peer) = self.peers.get(&peer_id) {
            matches!(peer.connection_status, ConnectionStatus::Authenticated)
                && peer.ping_failures < MAX_MISSED_PINGS
                && peer.last_seen.elapsed().as_secs() < PING_INTERVAL_SECONDS * 2
        } else {
            false
        }
    }

    pub fn get_connection_stats(&self) -> ConnectionStats {
        let mut stats = ConnectionStats::default();

        for (_, peer) in &self.peers {
            match peer.connection_status {
                ConnectionStatus::Authenticated => stats.authenticated += 1,
                ConnectionStatus::Connected => stats.connected += 1,
                ConnectionStatus::Connecting => stats.connecting += 1,
                ConnectionStatus::Reconnecting => stats.reconnecting += 1,
                ConnectionStatus::Disconnected => stats.disconnected += 1,
                ConnectionStatus::Error(_) => stats.error += 1,
            }

            if peer.ping_failures > 0 {
                stats.unhealthy += 1;
            }
        }

        stats.total = self.peers.len();
        stats
    }

    pub async fn handle_message(
        &mut self,
        peer_id: Uuid,
        message: Message,
        clipboard: &crate::clipboard::ClipboardManager,
    ) -> Result<()> {
        // Update last seen timestamp
        if let Some(peer) = self.peers.get_mut(&peer_id) {
            peer.last_seen = Instant::now();
        }

        match &message.message_type {
            MessageType::Ping => {
                debug!("Received ping from {}", peer_id);
                let pong = Message::pong();
                self.send_message_to_peer(peer_id, pong).await?;
            }

            MessageType::Pong => {
                debug!("Received pong from {}", peer_id);
                if let Some(peer) = self.peers.get_mut(&peer_id) {
                    peer.last_ping = Some(Instant::now());
                    peer.ping_failures = 0;
                }
            }

            MessageType::ClipboardUpdate {
                file_path,
                source_device,
                timestamp,
                file_size,
            } => {
                info!("Received clipboard update from {}: {}", peer_id, file_path);

                let clipboard_item = crate::clipboard::NetworkClipboardItem {
                    file_path: PathBuf::from(file_path),
                    source_device: *source_device,
                    timestamp: *timestamp,
                    file_size: *file_size,
                };

                clipboard.update_from_network(clipboard_item).await;
            }

            MessageType::ClipboardClear => {
                info!("Received clipboard clear from {}", peer_id);
                clipboard.clear().await;
            }

            MessageType::FileOffer {
                transfer_id,
                metadata,
            } => {
                info!(
                    "✅ Processing incoming FileOffer from {}: {}",
                    peer_id, metadata.name
                );

                let mut ft = self.file_transfer.write().await;
                ft.handle_file_offer(peer_id, *transfer_id, metadata.clone())
                    .await?;

                let response = ft.create_file_offer_response(*transfer_id, true, None);
                drop(ft);

                self.send_message_to_peer(peer_id, response).await?;
            }

            MessageType::FileChunk { transfer_id, chunk } => {
                let mut ft = self.file_transfer.write().await;
                ft.handle_file_chunk(peer_id, *transfer_id, chunk.clone())
                    .await?;
            }

            MessageType::TransferComplete {
                transfer_id,
                checksum,
            } => {
                info!(
                    "✅ Received TransferComplete for transfer {} from peer {}",
                    transfer_id, peer_id
                );

                let mut ft = self.file_transfer.write().await;
                ft.handle_transfer_complete(peer_id, *transfer_id, checksum.clone())
                    .await?;
            }

            MessageType::Handshake {
                device_id,
                device_name,
                version,
            } => {
                info!(
                    "🤝 Received handshake from {} ({}): {}",
                    device_name, device_id, version
                );

                // Always create or update peer entry for incoming handshakes
                // This handles the race condition where handshake arrives before discovery
                
                if let Some(peer) = self.peers.get_mut(device_id) {
                    // Update existing peer (from discovery or previous handshake)
                    info!("✅ Updating existing peer {}: {}", device_id, device_name);
                    peer.device_info.name = device_name.clone();
                    peer.connection_status = ConnectionStatus::Authenticated;
                    peer.ping_failures = 0;
                    peer.reconnection_attempts = 0;
                } else {
                    // Create new peer entry for incoming connection
                    // This is normal for incoming connections that arrive before discovery
                    info!("🆕 Creating peer entry for incoming handshake: {} ({})", device_name, device_id);
                    
                    let device_info = DeviceInfo {
                        id: *device_id,
                        name: device_name.clone(),
                        addr: "0.0.0.0:0".parse().unwrap(), // Will be updated by discovery later
                        last_seen: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs(),
                        version: version.clone(),
                    };
                    
                    let peer = Peer {
                        device_info,
                        connection_status: ConnectionStatus::Authenticated,
                        last_seen: Instant::now(),
                        last_ping: None,
                        ping_failures: 0,
                        reconnection_attempts: 0,
                    };
                    
                    self.peers.insert(*device_id, peer);
                }
                
                // Always move stream manager from temporary ID to real device ID
                if peer_id != *device_id {
                    if let Some(stream_manager) = self.stream_managers.remove(&peer_id) {
                        info!("📝 Moving stream manager from temporary ID {} to real ID {}", peer_id, device_id);
                        self.stream_managers.insert(*device_id, stream_manager);
                    }
                }

                // Send handshake response to the real device ID
                let response = Message::new(MessageType::HandshakeResponse {
                    accepted: true,
                    reason: None,
                });

                // Send handshake response - try real device ID first, then temporary peer_id
                let mut response_sent = false;
                
                // First try the real device ID (if stream manager was moved)
                if let Some(stream_manager) = self.stream_managers.get(device_id) {
                    if let Err(e) = stream_manager.send_control_message(response.clone()).await {
                        warn!("Failed to send handshake response via real ID {}: {}", device_id, e);
                    } else {
                        info!("✅ Sent handshake response to peer {} (real ID)", device_id);
                        response_sent = true;
                    }
                }
                
                // If that failed, try the temporary peer_id
                if !response_sent {
                    if let Some(stream_manager) = self.stream_managers.get(&peer_id) {
                        if let Err(e) = stream_manager.send_control_message(response).await {
                            error!("Failed to send handshake response via temporary ID {}: {}", peer_id, e);
                        } else {
                            info!("✅ Sent handshake response to peer {} (temporary ID)", peer_id);
                            response_sent = true;
                        }
                    }
                }
                
                if !response_sent {
                    error!("❌ No stream manager found for peer {} (real) or {} (temp) after handshake", device_id, peer_id);
                }
            }

            MessageType::HandshakeResponse { accepted, reason } => {
                if *accepted {
                    info!("✅ Handshake accepted by peer {}", peer_id);
                    
                    // Find the peer by peer_id (could be temporary or real ID)
                    let mut found_peer = false;
                    
                    // First try to find by the peer_id directly
                    if let Some(peer) = self.peers.get_mut(&peer_id) {
                        peer.connection_status = ConnectionStatus::Authenticated;
                        peer.ping_failures = 0;
                        peer.reconnection_attempts = 0;
                        found_peer = true;
                        info!("🔒 Peer {} authenticated via direct ID", peer_id);
                    } else {
                        // If not found, this might be a response where we need to find by stream manager
                        // Look through all peers to find one that matches this connection
                        for (real_peer_id, peer) in self.peers.iter_mut() {
                            if self.stream_managers.contains_key(real_peer_id) && peer.connection_status == ConnectionStatus::Connected {
                                peer.connection_status = ConnectionStatus::Authenticated;
                                peer.ping_failures = 0;
                                peer.reconnection_attempts = 0;
                                found_peer = true;
                                info!("🔒 Peer {} authenticated via connection mapping", real_peer_id);
                                break;
                            }
                        }
                    }
                    
                    if !found_peer {
                        warn!("⚠️ Could not find peer {} to authenticate", peer_id);
                    }
                } else {
                    warn!(
                        "❌ Handshake rejected by peer {}: {:?}",
                        peer_id, reason
                    );
                    if let Some(peer) = self.peers.get_mut(&peer_id) {
                        peer.connection_status = ConnectionStatus::Error(
                            reason.as_deref().unwrap_or("Handshake rejected").to_string(),
                        );
                    }
                }
            }

            _ => {
                debug!(
                    "Unhandled message type from {}: {:?}",
                    peer_id, message.message_type
                );
            }
        }

        Ok(())
    }
}
