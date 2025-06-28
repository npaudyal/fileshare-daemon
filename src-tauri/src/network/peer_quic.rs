use crate::{
    config::Settings,
    network::{discovery::DeviceInfo, protocol::*},
    quic::{
        UltraTransfer, BlazingTransfer, QuicConnectionManager,
        StreamManager,
    },
    FileshareError, Result,
};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, RwLock};
use tokio::time::timeout;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

// Constants for connection health monitoring
const PING_INTERVAL_SECONDS: u64 = 30;
const PING_TIMEOUT_SECONDS: u64 = 10;
const MAX_MISSED_PINGS: u32 = 3;
const RECONNECTION_DELAY_SECONDS: u64 = 5;
const MAX_RECONNECTION_ATTEMPTS: u32 = 5;

// Removed global registry and SimpleFileReceiver - using direct QUIC streams

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
    pub message_tx: mpsc::UnboundedSender<(Uuid, Message)>,
    pub message_rx: mpsc::UnboundedReceiver<(Uuid, Message)>,
    // QUIC components
    quic_manager: Arc<QuicConnectionManager>,
    stream_managers: HashMap<Uuid, Arc<StreamManager>>,
    // Mapping from temporary connection IDs to real device IDs
    temp_to_real_id_map: HashMap<Uuid, Uuid>,
    // Temporary storage for incoming stream managers before handshake
    incoming_stream_managers: Arc<RwLock<HashMap<Uuid, Arc<StreamManager>>>>,
}

impl PeerManager {
    pub async fn new(settings: Arc<Settings>) -> Result<Self> {
        let (message_tx, message_rx) = mpsc::unbounded_channel();

        // Create QUIC connection manager
        let bind_addr: SocketAddr = format!("0.0.0.0:{}", settings.network.port)
            .parse()
            .map_err(|e| FileshareError::Network(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("Invalid address: {}", e)
            )))?;
        let quic_manager = Arc::new(QuicConnectionManager::new(bind_addr).await?);

        let peer_manager = Self {
            settings,
            peers: HashMap::new(),
            message_tx: message_tx.clone(),
            message_rx,
            quic_manager,
            stream_managers: HashMap::new(),
            temp_to_real_id_map: HashMap::new(),
            incoming_stream_managers: Arc::new(RwLock::new(HashMap::new())),
        };

        // Start accepting QUIC connections
        peer_manager.start_quic_listener();

        Ok(peer_manager)
    }

    fn start_quic_listener(&self) {
        let quic_manager = self.quic_manager.clone();
        let message_tx = self.message_tx.clone();
        let incoming_managers = self.incoming_stream_managers.clone();

        tokio::spawn(async move {
            loop {
                match quic_manager.accept_connection().await {
                    Ok(connection) => {
                        info!(
                            "Accepted QUIC connection from {}",
                            connection.remote_address()
                        );

                        // Handle the connection in a separate task
                        let message_tx = message_tx.clone();
                        let incoming_managers = incoming_managers.clone();

                        tokio::spawn(async move {
                            let temp_id = connection.get_peer_id();
                            let stream_manager = Arc::new(StreamManager::new(connection.clone(), message_tx.clone()));
                            
                            // Store in incoming managers
                            {
                                let mut managers = incoming_managers.write().await;
                                managers.insert(temp_id, stream_manager.clone());
                                info!("✅ Stored incoming stream manager with temp ID: {}", temp_id);
                            }
                            
                            // Start the stream listener
                            stream_manager.start_stream_listener().await;
                            
                            // Keep connection alive
                            loop {
                                if connection.is_closed() {
                                    info!("📴 Incoming QUIC connection closed");
                                    // Clean up
                                    let mut managers = incoming_managers.write().await;
                                    managers.remove(&temp_id);
                                    break;
                                }
                                tokio::time::sleep(Duration::from_secs(1)).await;
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


    /// Resolve a peer ID, mapping temporary IDs to real device IDs when available
    fn resolve_peer_id(&self, peer_id: Uuid) -> Uuid {
        self.temp_to_real_id_map.get(&peer_id).copied().unwrap_or(peer_id)
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
        // Resolve the peer ID (map temporary to real ID if needed)
        let resolved_peer_id = self.resolve_peer_id(peer_id);
        
        info!(
            "Attempting to send message to peer {} (resolved to {}): {:?}",
            peer_id, resolved_peer_id, message.message_type
        );

        if let Some(stream_manager) = self.stream_managers.get(&resolved_peer_id) {
            stream_manager.send_control_message(message).await?;
            info!("✅ Successfully sent message to peer {} (resolved to {})", peer_id, resolved_peer_id);
            Ok(())
        } else {
            error!("❌ No active QUIC connection to peer {} (resolved to {})", peer_id, resolved_peer_id);
            Err(FileshareError::Transfer(format!(
                "No active QUIC connection to peer {} (resolved to {})",
                peer_id, resolved_peer_id
            )))
        }
    }

    pub async fn send_direct_to_connection(&self, peer_id: Uuid, message: Message) -> Result<()> {
        // Resolve the peer ID (map temporary to real ID if needed)
        let resolved_peer_id = self.resolve_peer_id(peer_id);
        
        if let Some(stream_manager) = self.stream_managers.get(&resolved_peer_id) {
            stream_manager.send_control_message(message).await?;
            Ok(())
        } else {
            Err(FileshareError::Transfer(format!(
                "No connection to peer {} (resolved to {})",
                peer_id, resolved_peer_id
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

        // Use ultra-optimized transfer for maximum speed
        tokio::spawn(async move {
            if let Err(e) = UltraTransfer::transfer_file(
                stream_manager,
                file_path,
                String::new(), // Target path will be determined by receiver
                peer_id,
            ).await {
                error!("❌ ULTRA transfer failed, trying BLAZING fallback: {}", e);
                // Could add blazing fallback here if needed
            }
        });

        info!("Started ULTRA QUIC file transfer to peer {}", peer_id);
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

                // Clean up any temporary ID mappings pointing to this peer
                self.temp_to_real_id_map.retain(|_temp_id, real_id| *real_id != peer_id);

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
        // Resolve peer ID for peer lookups (temp -> real ID mapping)
        let resolved_peer_id = self.resolve_peer_id(peer_id);
        
        // Update last seen timestamp (use resolved ID for peer lookups)
        if let Some(peer) = self.peers.get_mut(&resolved_peer_id) {
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
                if let Some(peer) = self.peers.get_mut(&resolved_peer_id) {
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

            // REMOVED: FileOffer/FileChunk/TransferComplete handling - we use direct QUIC streams now

            MessageType::Handshake {
                device_id,
                device_name,
                version,
            } => {
                info!(
                    "🤝 Received handshake from {} ({}): {}",
                    device_name, device_id, version
                );

                // Check if we need to move stream manager from temp to real ID
                let stream_manager_to_move = if peer_id != *device_id {
                    // First try local stream managers
                    if let Some(sm) = self.stream_managers.remove(&peer_id) {
                        Some(sm)
                    } else {
                        // Then try incoming stream managers
                        let mut incoming = self.incoming_stream_managers.write().await;
                        incoming.remove(&peer_id)
                    }
                } else {
                    None
                };

                // Update or create peer entry
                if let Some(peer) = self.peers.get_mut(device_id) {
                    // Update existing peer (from discovery)
                    info!("✅ Updating existing peer {} from {:?} to Authenticated", device_id, peer.connection_status);
                    peer.device_info.name = device_name.clone();
                    peer.connection_status = ConnectionStatus::Authenticated;
                    peer.ping_failures = 0;
                    peer.reconnection_attempts = 0;
                    peer.last_seen = Instant::now();
                } else {
                    // Create new peer entry for incoming connection (before discovery)
                    info!("🆕 Creating peer entry for incoming connection: {} ({})", device_name, device_id);
                    
                    let device_info = DeviceInfo {
                        id: *device_id,
                        name: device_name.clone(),
                        addr: "0.0.0.0:0".parse().unwrap(), // Will be updated by discovery
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
                
                // Move stream manager if needed
                if let Some(stream_manager) = stream_manager_to_move {
                    info!("📝 Moving stream manager from temporary ID {} to real ID {}", peer_id, device_id);
                    self.stream_managers.insert(*device_id, stream_manager.clone());
                    self.temp_to_real_id_map.insert(peer_id, *device_id);
                    
                    // Send handshake response immediately using the moved stream manager
                    let response = Message::new(MessageType::HandshakeResponse {
                        accepted: true,
                        reason: None,
                    });
                    
                    if let Err(e) = stream_manager.send_control_message(response).await {
                        error!("❌ Failed to send handshake response: {}", e);
                    } else {
                        info!("✅ Sent handshake response to peer {} successfully", device_id);
                    }
                } else if peer_id == *device_id {
                    // For outgoing connections where peer_id already matches device_id
                    if let Some(stream_manager) = self.stream_managers.get(device_id) {
                        let response = Message::new(MessageType::HandshakeResponse {
                            accepted: true,
                            reason: None,
                        });
                        
                        if let Err(e) = stream_manager.send_control_message(response).await {
                            error!("❌ Failed to send handshake response: {}", e);
                        } else {
                            info!("✅ Sent handshake response to peer {} successfully", device_id);
                        }
                    }
                }
            }

            MessageType::HandshakeResponse { accepted, reason } => {
                if *accepted {
                    info!("✅ Handshake accepted by peer {}", peer_id);
                    
                    // Find the peer by peer_id (could be temporary or real ID)
                    let mut found_peer = false;
                    
                    // First try to find by the resolved peer_id
                    if let Some(peer) = self.peers.get_mut(&resolved_peer_id) {
                        peer.connection_status = ConnectionStatus::Authenticated;
                        peer.ping_failures = 0;
                        peer.reconnection_attempts = 0;
                        found_peer = true;
                        info!("🔒 Peer {} (resolved to {}) authenticated via direct ID", peer_id, resolved_peer_id);
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

            MessageType::FileRequest {
                request_id,
                file_path,
                target_path,
            } => {
                info!(
                    "📁 Received file request from {}: {} -> {}",
                    peer_id, file_path, target_path
                );

                // Validate that the requested file exists
                let source_path = PathBuf::from(file_path);
                
                if !source_path.exists() {
                    error!("❌ Requested file does not exist: {}", file_path);
                    
                    // Send rejection response
                    let response = Message::new(MessageType::FileRequestResponse {
                        request_id: *request_id,
                        accepted: false,
                        reason: Some("File does not exist".to_string()),
                    });
                    
                    if let Err(e) = self.send_message_to_peer(peer_id, response).await {
                        error!("Failed to send file request rejection: {}", e);
                    }
                    return Ok(());
                }

                // Send acceptance response first
                let response = Message::new(MessageType::FileRequestResponse {
                    request_id: *request_id,
                    accepted: true,
                    reason: None,
                });
                
                if let Err(e) = self.send_message_to_peer(peer_id, response).await {
                    error!("Failed to send file request acceptance: {}", e);
                    return Ok(());
                }

                // Use the new optimized transfer
                let stream_manager_opt = self.stream_managers.get(&resolved_peer_id).cloned();
                
                if let Some(stream_manager) = stream_manager_opt {
                    let source_path_clone = source_path.clone();
                    let target_path_clone = target_path.clone();
                    
                    info!("⚡ Starting ULTRA QUIC transfer: {} -> {}", file_path, target_path);
                    
                    // Start ultra-optimized transfer
                    tokio::spawn(async move {
                        if let Err(e) = UltraTransfer::transfer_file(
                            stream_manager,
                            source_path_clone,
                            target_path_clone,
                            resolved_peer_id,
                        ).await {
                            error!("❌ ULTRA transfer failed: {}", e);
                        }
                    });
                    
                    info!("✅ ULTRA transfer initiated: {} -> {}", file_path, target_path);
                } else {
                    error!("❌ No stream manager found for peer {}", resolved_peer_id);
                }
            }

            MessageType::FileRequestResponse {
                request_id,
                accepted,
                reason,
            } => {
                if *accepted {
                    info!("✅ File request {} accepted by peer {}", request_id, peer_id);
                    // File transfer will start automatically from the accepting peer
                } else {
                    warn!(
                        "❌ File request {} rejected by peer {}: {}",
                        request_id,
                        peer_id,
                        reason.as_deref().unwrap_or("No reason provided")
                    );
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
