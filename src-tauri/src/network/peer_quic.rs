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
use std::sync::{Arc, Mutex};
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

// Global registry for incoming stream managers
lazy_static::lazy_static! {
    static ref INCOMING_STREAM_MANAGERS: Mutex<HashMap<Uuid, Arc<StreamManager>>> = Mutex::new(HashMap::new());
}

// Simple file receiver for direct QUIC transfers
struct SimpleFileReceiver {
    metadata: FileMetadata,
    file_path: PathBuf,
    writer: Option<tokio::fs::File>,
    bytes_received: u64,
    chunks_received: Vec<bool>,
}

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
    // Mapping from temporary connection IDs to real device IDs
    temp_to_real_id_map: HashMap<Uuid, Uuid>,
    // Simple file receivers for incoming transfers
    active_receivers: Arc<RwLock<HashMap<Uuid, SimpleFileReceiver>>>,
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
            temp_to_real_id_map: HashMap::new(),
            active_receivers: Arc::new(RwLock::new(HashMap::new())),
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
        
        // Get the temporary peer ID for this connection
        let temp_peer_id = connection.get_peer_id();
        info!("📋 Generated temporary peer ID for incoming connection: {}", temp_peer_id);
        
        // Create stream manager for this connection
        let stream_manager = Arc::new(StreamManager::new(connection.clone(), message_tx.clone()));

        // Register the stream manager globally so the handshake handler can find it
        {
            let mut registry = INCOMING_STREAM_MANAGERS.lock().unwrap();
            registry.insert(temp_peer_id, stream_manager.clone());
            info!("✅ Stream manager registered with temporary ID: {}", temp_peer_id);
        }

        // Start stream listener - this will handle incoming messages and forward them to message_tx
        stream_manager.clone().start_stream_listener().await;

        info!("✅ Stream manager started for incoming connection, handshake will be handled by main message handler");
        
        // Keep the connection alive by monitoring connection status
        // The stream manager handles all message processing in background tasks
        loop {
            if connection.is_closed() {
                info!("📴 Incoming QUIC connection closed");
                
                // Clean up the stream manager from global registry
                {
                    let mut registry = INCOMING_STREAM_MANAGERS.lock().unwrap();
                    if registry.remove(&temp_peer_id).is_some() {
                        info!("🗑️ Cleaned up stream manager for temporary ID: {}", temp_peer_id);
                    }
                }
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

            MessageType::FileOffer {
                transfer_id,
                metadata,
            } => {
                info!(
                    "✅ Processing incoming FileOffer from {}: {} ({} bytes)",
                    peer_id, metadata.name, metadata.size
                );

                // Create a simple file receiver
                let save_dir = self.get_default_save_dir();
                let file_path = save_dir.join(&metadata.name);
                
                let receiver = SimpleFileReceiver {
                    metadata: metadata.clone(),
                    file_path: file_path.clone(),
                    writer: None,
                    bytes_received: 0,
                    chunks_received: vec![false; metadata.total_chunks as usize],
                };
                
                // Store the receiver
                {
                    let mut receivers = self.active_receivers.write().await;
                    receivers.insert(*transfer_id, receiver);
                }
                
                // Send acceptance
                let response = Message::new(MessageType::FileOfferResponse {
                    transfer_id: *transfer_id,
                    accepted: true,
                    reason: None,
                });

                self.send_message_to_peer(peer_id, response).await?;
                
                info!("✅ Accepted FileOffer {} from peer {}, will save to {:?}", 
                      transfer_id, peer_id, file_path);
            }

            MessageType::FileChunk { transfer_id, chunk } => {
                // Handle chunk directly with simple receiver
                self.handle_chunk_direct(*transfer_id, chunk.clone()).await?;
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
                        // Store the mapping from temporary ID to real device ID
                        self.temp_to_real_id_map.insert(peer_id, *device_id);
                        info!("🔗 Stored temp-to-real ID mapping: {} -> {}", peer_id, device_id);
                    }
                }

                // Send handshake response to the real device ID
                let response = Message::new(MessageType::HandshakeResponse {
                    accepted: true,
                    reason: None,
                });

                // Send handshake response - try multiple strategies
                let mut response_sent = false;
                
                // Strategy 1: Try the real device ID (if stream manager was moved)
                if let Some(stream_manager) = self.stream_managers.get(device_id) {
                    if let Err(e) = stream_manager.send_control_message(response.clone()).await {
                        warn!("Failed to send handshake response via real ID {}: {}", device_id, e);
                    } else {
                        info!("✅ Sent handshake response to peer {} (real ID)", device_id);
                        response_sent = true;
                    }
                }
                
                // Strategy 2: Try the temporary peer_id in regular stream managers
                if !response_sent {
                    if let Some(stream_manager) = self.stream_managers.get(&peer_id) {
                        if let Err(e) = stream_manager.send_control_message(response.clone()).await {
                            error!("Failed to send handshake response via temporary ID {}: {}", peer_id, e);
                        } else {
                            info!("✅ Sent handshake response to peer {} (temporary ID)", peer_id);
                            response_sent = true;
                        }
                    }
                }
                
                // Strategy 3: Try the global incoming stream manager registry
                if !response_sent {
                    let stream_manager_opt = {
                        let registry = INCOMING_STREAM_MANAGERS.lock().unwrap();
                        registry.get(&peer_id).cloned()
                    };
                    
                    if let Some(stream_manager) = stream_manager_opt {
                        if let Err(e) = stream_manager.send_control_message(response.clone()).await {
                            error!("Failed to send handshake response via global registry {}: {}", peer_id, e);
                        } else {
                            info!("✅ Sent handshake response to peer {} (global registry)", peer_id);
                            response_sent = true;
                            
                            // Move the stream manager from global registry to local storage
                            let mut registry = INCOMING_STREAM_MANAGERS.lock().unwrap();
                            if let Some(stream_manager) = registry.remove(&peer_id) {
                                self.stream_managers.insert(*device_id, stream_manager);
                                // Store the mapping from temporary ID to real device ID
                                self.temp_to_real_id_map.insert(peer_id, *device_id);
                                info!("📝 Moved stream manager from global registry to local storage for device {}", device_id);
                                info!("🔗 Stored temp-to-real ID mapping: {} -> {}", peer_id, device_id);
                            }
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

                // Validate that the requested file exists and send it
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

                // SIMPLIFIED: Start QUIC file transfer directly without going through FileTransferManager
                let stream_manager_opt = self.stream_managers.get(&resolved_peer_id).cloned();
                
                if let Some(stream_manager) = stream_manager_opt {
                    let transfer_id = Uuid::new_v4();
                    let source_path_clone = source_path.clone();
                    
                    // Get file metadata
                    let file_size = match tokio::fs::metadata(&source_path).await {
                        Ok(metadata) => metadata.len(),
                        Err(e) => {
                            error!("Failed to get file metadata: {}", e);
                            return Ok(());
                        }
                    };
                    
                    let chunk_size = crate::service::streaming::calculate_adaptive_chunk_size(file_size);
                    let metadata = match FileMetadata::from_path_with_chunk_size(&source_path, chunk_size) {
                        Ok(metadata) => metadata.with_target_dir(Some(target_path.clone())),
                        Err(e) => {
                            error!("Failed to create file metadata: {}", e);
                            return Ok(());
                        }
                    };
                    
                    // Send FileOffer first
                    let offer = Message::new(MessageType::FileOffer {
                        transfer_id,
                        metadata: metadata.clone(),
                    });
                    
                    if let Err(e) = stream_manager.send_control_message(offer).await {
                        error!("Failed to send file offer: {}", e);
                        return Ok(());
                    }
                    
                    info!("📤 Sent FileOffer for transfer {} to peer {}", transfer_id, resolved_peer_id);
                    
                    // Start the QUIC transfer directly
                    tokio::spawn(async move {
                        // Wait a bit for offer acceptance
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        
                        let quic_transfer = crate::quic::QuicFileTransfer::new(
                            stream_manager,
                            resolved_peer_id,
                            transfer_id,
                        );
                        
                        match quic_transfer.send_file_parallel(source_path_clone, metadata).await {
                            Ok(()) => {
                                info!("✅ QUIC file transfer {} completed successfully", transfer_id);
                            }
                            Err(e) => {
                                error!("❌ QUIC file transfer {} failed: {}", transfer_id, e);
                            }
                        }
                    });
                    
                    info!("✅ File transfer initiated for request {}: {} -> {}", request_id, file_path, target_path);
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
    
    fn get_default_save_dir(&self) -> PathBuf {
        if let Some(ref temp_dir) = self.settings.transfer.temp_dir {
            temp_dir.clone()
        } else {
            directories::UserDirs::new()
                .and_then(|dirs| dirs.download_dir().map(|d| d.to_path_buf()))
                .unwrap_or_else(|| {
                    directories::UserDirs::new()
                        .and_then(|dirs| dirs.document_dir().map(|d| d.to_path_buf()))
                        .unwrap_or_else(|| PathBuf::from("."))
                })
        }
    }
    
    async fn handle_chunk_direct(&self, transfer_id: Uuid, chunk: TransferChunk) -> Result<()> {
        // Check if we need to initialize the file writer
        let file_path = {
            let mut receivers = self.active_receivers.write().await;
            if let Some(receiver) = receivers.get_mut(&transfer_id) {
                if receiver.writer.is_none() {
                    Some(receiver.file_path.clone())
                } else {
                    None
                }
            } else {
                warn!("Received chunk for unknown transfer: {}", transfer_id);
                return Ok(());
            }
        };
        
        // Initialize file writer if needed (outside the lock)
        if let Some(file_path) = file_path {
            // Create parent directories if needed
            if let Some(parent) = file_path.parent() {
                tokio::fs::create_dir_all(parent).await.map_err(|e| {
                    FileshareError::FileOperation(format!("Failed to create directory: {}", e))
                })?;
            }
            
            let file = tokio::fs::File::create(&file_path).await.map_err(|e| {
                FileshareError::FileOperation(format!("Failed to create file: {}", e))
            })?;
            
            let mut receivers = self.active_receivers.write().await;
            if let Some(receiver) = receivers.get_mut(&transfer_id) {
                receiver.writer = Some(file);
            }
        }
        
        // Process the chunk
        let should_remove = {
            let mut receivers = self.active_receivers.write().await;
            if let Some(receiver) = receivers.get_mut(&transfer_id) {
                // Mark chunk as received
                if chunk.index < receiver.chunks_received.len() as u64 {
                    receiver.chunks_received[chunk.index as usize] = true;
                    receiver.bytes_received += chunk.data.len() as u64;
                    
                    // Write chunk to file (simplified - just append for now)
                    if let Some(ref mut writer) = receiver.writer {
                        use tokio::io::AsyncWriteExt;
                        writer.write_all(&chunk.data).await.map_err(|e| {
                            FileshareError::FileOperation(format!("Failed to write chunk: {}", e))
                        })?;
                    }
                    
                    // Check if transfer is complete
                    let all_received = receiver.chunks_received.iter().all(|&received| received);
                    let should_complete = all_received || chunk.is_last;
                    
                    if should_complete {
                        info!("✅ File transfer {} completed: {:?}", transfer_id, receiver.file_path);
                        
                        // Flush and close file
                        if let Some(ref mut writer) = receiver.writer {
                            use tokio::io::AsyncWriteExt;
                            writer.flush().await.map_err(|e| {
                                FileshareError::FileOperation(format!("Failed to flush file: {}", e))
                            })?;
                        }
                    }
                    
                    if chunk.index % 50 == 0 || chunk.is_last {
                        let progress = (receiver.bytes_received as f64 / receiver.metadata.size as f64) * 100.0;
                        info!("📊 Transfer {}: {:.1}% complete ({} bytes)", 
                              transfer_id, progress, receiver.bytes_received);
                    }
                    
                    should_complete
                } else {
                    false
                }
            } else {
                false
            }
        };
        
        // Remove completed receiver (outside the processing lock)
        if should_remove {
            let mut receivers = self.active_receivers.write().await;
            receivers.remove(&transfer_id);
        }
        
        Ok(())
    }
}
