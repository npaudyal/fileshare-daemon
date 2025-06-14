use crate::{
    config::Settings,
    network::{discovery::DeviceInfo, protocol::*},
    service::file_transfer::{
        FileTransferManager, MessageSender, TransferDirection, TransferStatus,
    },
    FileshareError, Result,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, RwLock};
use tokio::time::{interval, timeout};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

// Constants for connection health monitoring
const PING_INTERVAL_SECONDS: u64 = 30; // Ping every 30 seconds
const PING_TIMEOUT_SECONDS: u64 = 10; // 10 second ping timeout
const MAX_MISSED_PINGS: u32 = 3; // Disconnect after 3 missed pings
const RECONNECTION_DELAY_SECONDS: u64 = 5; // Wait 5 seconds before reconnecting
const MAX_RECONNECTION_ATTEMPTS: u32 = 5; // Try reconnecting 5 times

#[derive(Debug, Clone)]
pub struct Peer {
    pub device_info: DeviceInfo,
    pub connection_status: ConnectionStatus,
    pub last_ping: Option<std::time::Instant>,
    pub ping_failures: u32,            // Track consecutive ping failures
    pub last_seen: std::time::Instant, // Track when we last heard from this peer
    pub reconnection_attempts: u32,    // Track reconnection attempts
}

#[derive(Debug, Clone)]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
    Authenticated,
    Reconnecting, // Reconnecting state
    Error(String),
}

// Connection statistics structure
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
    peers: HashMap<Uuid, Peer>,
    pub file_transfer: Arc<RwLock<FileTransferManager>>,
    pub message_tx: mpsc::UnboundedSender<(Uuid, Message)>,
    pub message_rx: mpsc::UnboundedReceiver<(Uuid, Message)>,
    // Store active connections to send messages
    connections: HashMap<Uuid, mpsc::UnboundedSender<Message>>,
}

impl PeerManager {
    pub async fn get_all_discovered_devices(&self) -> Vec<crate::network::discovery::DeviceInfo> {
        // Return peers as discovered devices for UI compatibility
        let mut discovered = Vec::new();

        for (_, peer) in &self.peers {
            discovered.push(crate::network::discovery::DeviceInfo {
                id: peer.device_info.id,
                name: peer.device_info.name.clone(),
                addr: peer.device_info.addr,
                last_seen: peer.device_info.last_seen,
                version: peer.device_info.version.clone(),
            });
        }

        discovered
    }

    pub fn get_peer_address(&self, peer_id: Uuid) -> Option<std::net::SocketAddr> {
        self.peers.get(&peer_id).map(|peer| peer.device_info.addr)
    }
    pub fn is_peer_connected(&self, peer_id: Uuid) -> bool {
        self.peers
            .get(&peer_id)
            .map(|peer| matches!(peer.connection_status, ConnectionStatus::Authenticated))
            .unwrap_or(false)
    }

    pub async fn new(settings: Arc<Settings>) -> Result<Self> {
        let (message_tx, message_rx) = mpsc::unbounded_channel();

        // Create file transfer manager WITHOUT any circular reference
        let mut file_transfer = FileTransferManager::new(settings.clone()).await?;

        // Set up the message sender
        file_transfer.set_message_sender(message_tx.clone());

        // PRODUCTION: Return PeerManager directly - no Arc wrapping
        Ok(Self {
            settings,
            peers: HashMap::new(),
            file_transfer: Arc::new(RwLock::new(file_transfer)),
            message_tx,
            message_rx,
            connections: HashMap::new(),
        })
    }

    pub async fn setup_file_transfer_callbacks(&mut self) {
        // Create callbacks that capture the peer information
        let peers_for_address = self.peers.clone();
        let peers_for_connected = self.peers.clone();

        let address_callback: crate::service::file_transfer::PeerAddressCallback =
            Arc::new(move |peer_id| {
                peers_for_address
                    .get(&peer_id)
                    .map(|peer| peer.device_info.addr)
            });

        let connected_callback: crate::service::file_transfer::PeerConnectedCallback =
            Arc::new(move |peer_id| {
                peers_for_connected
                    .get(&peer_id)
                    .map(|peer| matches!(peer.connection_status, ConnectionStatus::Authenticated))
                    .unwrap_or(false)
            });

        // Set the callbacks
        let mut ft = self.file_transfer.write().await;
        ft.set_peer_callbacks(address_callback, connected_callback);

        info!("✅ File transfer callbacks configured");
    }
    pub fn debug_connection_status(&self) {
        info!("=== CONNECTION STATUS DEBUG ===");
        info!("Total discovered peers: {}", self.peers.len());
        info!("Active connections: {}", self.connections.len());

        for (peer_id, peer) in &self.peers {
            info!(
                "Peer {}: {} - Status: {:?} - Ping failures: {} - Reconnection attempts: {}",
                peer_id,
                peer.device_info.name,
                peer.connection_status,
                peer.ping_failures,
                peer.reconnection_attempts
            );
        }

        for (peer_id, _) in &self.connections {
            info!("Active connection to: {}", peer_id);
        }
        info!("=== END CONNECTION STATUS ===");
    }

    // Add this method for direct connection sending (used by daemon routing)
    pub async fn send_direct_to_connection(&self, peer_id: Uuid, message: Message) -> Result<()> {
        if let Some(conn) = self.connections.get(&peer_id) {
            conn.send(message).map_err(|e| {
                FileshareError::Transfer(format!("Failed to send direct message: {}", e))
            })?;
            Ok(())
        } else {
            Err(FileshareError::Transfer(format!(
                "No connection to peer {}",
                peer_id
            )))
        }
    }

    // Simplified message sending
    pub async fn send_message_to_peer(&mut self, peer_id: Uuid, message: Message) -> Result<()> {
        info!(
            "Attempting to send message to peer {}: {:?}",
            peer_id, message.message_type
        );

        if let Some(conn) = self.connections.get(&peer_id) {
            match conn.send(message) {
                Ok(()) => {
                    info!("✅ Successfully sent message to peer {}", peer_id);
                    Ok(())
                }
                Err(e) => {
                    error!("❌ Failed to send message to peer {}: {}", peer_id, e);
                    Err(FileshareError::Transfer(format!(
                        "Failed to send message to peer: {}",
                        e
                    )))
                }
            }
        } else {
            error!("❌ No active connection to peer {}", peer_id);
            self.debug_connection_status();
            Err(FileshareError::Transfer(format!(
                "No active connection to peer {}",
                peer_id
            )))
        }
    }

    pub async fn on_device_discovered(&mut self, device_info: DeviceInfo) -> Result<()> {
        // Check if peer already exists
        if let Some(existing_peer) = self.peers.get_mut(&device_info.id) {
            // Update last seen time and device info, but keep connection status
            existing_peer.device_info = device_info;
            existing_peer.last_seen = Instant::now();
            debug!(
                "Updated existing peer: {} ({})",
                existing_peer.device_info.name, existing_peer.device_info.id
            );
            return Ok(());
        }

        // Create new peer only if it doesn't exist
        let peer = Peer {
            device_info: device_info.clone(),
            connection_status: ConnectionStatus::Disconnected,
            last_ping: None,
            ping_failures: 0,
            last_seen: Instant::now(),
            reconnection_attempts: 0,
        };

        info!("Adding new peer: {} ({})", device_info.name, device_info.id);
        self.peers.insert(device_info.id, peer);

        // Attempt to connect to new peers
        if self.settings.security.require_pairing {
            if self
                .settings
                .security
                .allowed_devices
                .contains(&device_info.id)
            {
                info!(
                    "Device {} is in allowed list, connecting...",
                    device_info.id
                );
                self.connect_to_peer(device_info.id).await?;
            } else {
                info!(
                    "Device {} not in allowed list, skipping connection",
                    device_info.id
                );
            }
        } else {
            info!(
                "Pairing not required, connecting to device {}",
                device_info.id
            );
            self.connect_to_peer(device_info.id).await?;
        }

        Ok(())
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

        info!("Connecting to peer {} at {}", peer_id, addr);

        match TcpStream::connect(addr).await {
            Ok(stream) => {
                info!("Successfully connected to peer {} at {}", peer_id, addr);
                peer.connection_status = ConnectionStatus::Connected;
                peer.last_seen = Instant::now();
                self.handle_peer_connection(peer_id, stream).await?;
            }
            Err(e) => {
                warn!("Failed to connect to peer {}: {}", peer_id, e);
                peer.connection_status = ConnectionStatus::Error(e.to_string());
            }
        }

        Ok(())
    }

    pub async fn handle_connection(&mut self, stream: TcpStream) -> Result<()> {
        let addr = stream.peer_addr()?;
        info!("Handling incoming connection from {}", addr);
        self.handle_unknown_connection(stream).await
    }

    async fn handle_unknown_connection(&mut self, stream: TcpStream) -> Result<()> {
        let mut connection = PeerConnection::new(stream);

        // Wait for handshake
        match connection.read_message().await? {
            Message {
                message_type:
                    MessageType::Handshake {
                        device_id,
                        device_name,
                        version,
                    },
                ..
            } => {
                info!("Received handshake from {} ({})", device_name, device_id);

                // Check if this device is allowed
                let accepted = if self.settings.security.require_pairing {
                    self.settings.security.allowed_devices.contains(&device_id)
                } else {
                    true
                };

                let response = if accepted {
                    Message::new(MessageType::HandshakeResponse {
                        accepted: true,
                        reason: None,
                    })
                } else {
                    Message::new(MessageType::HandshakeResponse {
                        accepted: false,
                        reason: Some("Device not paired".to_string()),
                    })
                };

                connection.write_message(&response).await?;

                if accepted {
                    info!("Handshake accepted for device {}", device_id);
                    // Update or create peer
                    if let Some(peer) = self.peers.get_mut(&device_id) {
                        peer.connection_status = ConnectionStatus::Authenticated;
                        peer.last_seen = Instant::now();
                        peer.ping_failures = 0;
                        peer.reconnection_attempts = 0;
                    } else {
                        // Create new peer from handshake info
                        let device_info = DeviceInfo {
                            id: device_id,
                            name: device_name,
                            addr: connection.stream.peer_addr()?,
                            last_seen: std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap()
                                .as_secs(),
                            version,
                        };

                        let peer = Peer {
                            device_info,
                            connection_status: ConnectionStatus::Authenticated,
                            last_ping: None,
                            ping_failures: 0,
                            last_seen: Instant::now(),
                            reconnection_attempts: 0,
                        };

                        self.peers.insert(device_id, peer);
                    }

                    // Handle the authenticated connection
                    self.handle_authenticated_connection(device_id, connection)
                        .await?;
                } else {
                    info!("Handshake rejected for device {}", device_id);
                }
            }
            _ => {
                warn!("Expected handshake message, got something else");
                return Err(FileshareError::Authentication(
                    "Invalid handshake".to_string(),
                ));
            }
        }

        Ok(())
    }

    async fn handle_peer_connection(&mut self, peer_id: Uuid, stream: TcpStream) -> Result<()> {
        let mut connection = PeerConnection::new(stream);

        // Send handshake
        let handshake =
            Message::handshake(self.settings.device.id, self.settings.device.name.clone());

        info!("Sending handshake to peer {}", peer_id);
        connection.write_message(&handshake).await?;

        // Wait for response
        match connection.read_message().await? {
            Message {
                message_type: MessageType::HandshakeResponse { accepted: true, .. },
                ..
            } => {
                info!("Handshake accepted by peer {}", peer_id);
                if let Some(peer) = self.peers.get_mut(&peer_id) {
                    peer.connection_status = ConnectionStatus::Authenticated;
                    peer.last_seen = Instant::now();
                    peer.ping_failures = 0;
                    peer.reconnection_attempts = 0;
                }
                self.handle_authenticated_connection(peer_id, connection)
                    .await?;
            }
            Message {
                message_type:
                    MessageType::HandshakeResponse {
                        accepted: false,
                        reason,
                    },
                ..
            } => {
                warn!("Handshake rejected by peer {}: {:?}", peer_id, reason);
                if let Some(peer) = self.peers.get_mut(&peer_id) {
                    peer.connection_status = ConnectionStatus::Error(
                        reason.unwrap_or_else(|| "Handshake rejected".to_string()),
                    );
                }
            }
            _ => {
                warn!("Expected handshake response, got something else");
                return Err(FileshareError::Authentication(
                    "Invalid handshake response".to_string(),
                ));
            }
        }

        Ok(())
    }

    async fn handle_authenticated_connection(
        &mut self,
        peer_id: Uuid,
        connection: PeerConnection,
    ) -> Result<()> {
        info!("Handling authenticated connection with peer {}", peer_id);

        let message_tx = self.message_tx.clone();
        let (conn_tx, mut conn_rx) = mpsc::unbounded_channel::<Message>();

        // Store the connection sender so we can send messages to this peer
        self.connections.insert(peer_id, conn_tx);

        // Split the connection into read and write halves
        let (mut read_half, mut write_half) = connection.split();

        // Spawn task to handle reading messages FROM the peer
        let read_message_tx = message_tx.clone();
        let read_peer_id = peer_id;
        let read_task = tokio::spawn(async move {
            loop {
                match read_half.read_message().await {
                    Ok(message) => {
                        info!(
                            "📥 READ from peer {}: {:?}",
                            read_peer_id, message.message_type
                        );
                        if let Err(e) = read_message_tx.send((read_peer_id, message)) {
                            error!(
                                "Failed to forward message from peer {}: {}",
                                read_peer_id, e
                            );
                            break;
                        }
                    }
                    Err(e) => {
                        warn!("Connection read error with peer {}: {}", read_peer_id, e);
                        break;
                    }
                }
            }
            info!("Read task ended for peer {}", read_peer_id);
        });

        // Spawn task to handle writing messages TO the peer
        let write_peer_id = peer_id;
        let write_task = tokio::spawn(async move {
            while let Some(message) = conn_rx.recv().await {
                info!(
                    "📤 WRITE to peer {}: {:?}",
                    write_peer_id, message.message_type
                );

                if let Err(e) = write_half.write_message(&message).await {
                    error!("Failed to write message to peer {}: {}", write_peer_id, e);
                    break;
                }
            }
            info!("Write task ended for peer {}", write_peer_id);
        });

        // Clean up when connection ends
        let cleanup_peer_id = peer_id;
        let connections_cleanup = self.connections.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = read_task => {
                    info!("Read task completed for peer {}", cleanup_peer_id);
                },
                _ = write_task => {
                    info!("Write task completed for peer {}", cleanup_peer_id);
                },
            }
            info!("Connection handler for peer {} ended", cleanup_peer_id);
        });

        Ok(())
    }

    pub async fn send_file_to_peer(
        &mut self,
        peer_id: Uuid,
        file_path: std::path::PathBuf,
    ) -> Result<()> {
        // Check if peer is connected and authenticated
        let peer = self
            .peers
            .get(&peer_id)
            .ok_or_else(|| FileshareError::Unknown("Peer not found".to_string()))?;

        if !matches!(peer.connection_status, ConnectionStatus::Authenticated) {
            return Err(FileshareError::Transfer(
                "Peer not authenticated".to_string(),
            ));
        }

        // Start file transfer with validation
        let mut ft = self.file_transfer.write().await;
        ft.send_file_with_validation(peer_id, file_path).await
    }

    pub fn get_connected_peers(&self) -> Vec<Peer> {
        self.peers
            .values()
            .filter(|peer| matches!(peer.connection_status, ConnectionStatus::Authenticated))
            .cloned()
            .collect()
    }

    // Check health of all peers
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

    // Check individual peer health
    async fn check_peer_health(&mut self, peer_id: Uuid) -> Result<()> {
        info!("🩺 Checking health of peer {}", peer_id);

        // Send ping and wait for response
        match self.ping_peer_with_timeout(peer_id).await {
            Ok(response_time) => {
                info!(
                    "✅ Peer {} responded to ping in {:?}",
                    peer_id, response_time
                );

                // Reset ping failures on successful ping
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

    // Ping peer with timeout
    async fn ping_peer_with_timeout(&mut self, peer_id: Uuid) -> Result<Duration> {
        let start_time = Instant::now();
        let ping_message = Message::ping();

        // Send ping
        self.send_message_to_peer(peer_id, ping_message).await?;

        // Wait for pong response with timeout
        match timeout(
            Duration::from_secs(PING_TIMEOUT_SECONDS),
            self.wait_for_pong(peer_id),
        )
        .await
        {
            Ok(_) => {
                let response_time = start_time.elapsed();
                Ok(response_time)
            }
            Err(_) => Err(FileshareError::Network(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "Ping timeout",
            ))),
        }
    }

    // Wait for pong response
    async fn wait_for_pong(&mut self, _peer_id: Uuid) -> Result<()> {
        // This would typically wait for a pong message
        // For now, we'll implement a simple delay simulation
        // In a real implementation, you'd wait for the actual pong message
        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    }

    // Handle ping failure
    async fn handle_ping_failure(&mut self, peer_id: Uuid) -> Result<()> {
        if let Some(peer) = self.peers.get_mut(&peer_id) {
            peer.ping_failures += 1;

            info!(
                "❌ Ping failure {} for peer {}",
                peer.ping_failures, peer_id
            );

            if peer.ping_failures >= MAX_MISSED_PINGS {
                warn!(
                    "💔 Peer {} exceeded max ping failures, marking as disconnected",
                    peer_id
                );
                peer.connection_status = ConnectionStatus::Disconnected;

                // Remove from active connections
                self.connections.remove(&peer_id);

                // Attempt reconnection if this was an authenticated peer
                self.attempt_reconnection(peer_id).await?;
            }
        }

        Ok(())
    }

    // Fixed version - separate the data extraction from the method calls
    async fn attempt_reconnection(&mut self, peer_id: Uuid) -> Result<()> {
        // First, extract the necessary data and update the peer state
        let should_reconnect = if let Some(peer) = self.peers.get_mut(&peer_id) {
            if peer.reconnection_attempts >= MAX_RECONNECTION_ATTEMPTS {
                error!("💥 Max reconnection attempts reached for peer {}", peer_id);
                peer.connection_status =
                    ConnectionStatus::Error("Max reconnection attempts exceeded".to_string());
                return Ok(());
            }

            peer.reconnection_attempts += 1;
            peer.connection_status = ConnectionStatus::Reconnecting;

            info!(
                "🔄 Attempting reconnection {} to peer {}",
                peer.reconnection_attempts, peer_id
            );

            true // Indicate we should proceed with reconnection
        } else {
            false // Peer not found
        };

        if should_reconnect {
            // Wait before reconnecting
            tokio::time::sleep(Duration::from_secs(RECONNECTION_DELAY_SECONDS)).await;

            // Now we can safely call connect_to_peer since we're not holding any borrows
            match self.connect_to_peer(peer_id).await {
                Ok(_) => {
                    info!("✅ Successfully reconnected to peer {}", peer_id);
                    // Reset reconnection attempts on successful connection
                    if let Some(peer) = self.peers.get_mut(&peer_id) {
                        peer.reconnection_attempts = 0;
                    }
                }
                Err(e) => {
                    // Get the current attempt count for logging
                    let attempt_count = self
                        .peers
                        .get(&peer_id)
                        .map(|p| p.reconnection_attempts)
                        .unwrap_or(0);

                    warn!(
                        "❌ Reconnection attempt {} failed for peer {}: {}",
                        attempt_count, peer_id, e
                    );
                }
            }
        }

        Ok(())
    }

    // Check if peer is healthy
    pub fn is_peer_healthy(&self, peer_id: Uuid) -> bool {
        if let Some(peer) = self.peers.get(&peer_id) {
            matches!(peer.connection_status, ConnectionStatus::Authenticated)
                && peer.ping_failures < MAX_MISSED_PINGS
                && peer.last_seen.elapsed().as_secs() < PING_INTERVAL_SECONDS * 2
        } else {
            false
        }
    }

    // Get connection statistics
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

    // Enhanced message handling with connection health updates
    pub async fn handle_message(
        &mut self,
        peer_id: Uuid,
        message: Message,
        clipboard: &crate::clipboard::ClipboardManager,
    ) -> Result<()> {
        // Update last seen timestamp for any message
        if let Some(peer) = self.peers.get_mut(&peer_id) {
            peer.last_seen = Instant::now();
        }

        info!(
            "📥 Processing message from {}: {:?}",
            peer_id, message.message_type
        );

        match message.message_type {
            MessageType::Ping => {
                debug!("Received ping from {}", peer_id);
                if let Some(conn) = self.connections.get(&peer_id) {
                    let _ = conn.send(Message::pong());
                }
            }
            MessageType::Pong => {
                debug!("Received pong from {}", peer_id);
                if let Some(peer) = self.peers.get_mut(&peer_id) {
                    peer.last_ping = Some(Instant::now());
                    peer.ping_failures = 0; // Reset ping failures on pong
                    peer.last_seen = Instant::now();
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
                    source_device,
                    timestamp,
                    file_size,
                };

                clipboard.update_from_network(clipboard_item).await;
            }

            MessageType::ClipboardClear => {
                info!("Received clipboard clear from {}", peer_id);
                clipboard.clear().await;
            }

            MessageType::StreamingFileOffer {
                transfer_id,
                metadata,
            } => {
                info!(
                    "✅ Processing StreamingFileOffer from {}: {} ({:.1}MB)",
                    peer_id,
                    metadata.name,
                    metadata.size as f64 / (1024.0 * 1024.0)
                );

                let mut ft = self.file_transfer.write().await;
                ft.handle_streaming_file_offer(peer_id, transfer_id, metadata)
                    .await?;
            }

            MessageType::StreamingFileOfferResponse {
                transfer_id,
                accepted,
                reason,
                suggested_config,
            } => {
                info!(
                    "Received streaming offer response from {}: {}",
                    peer_id, accepted
                );

                if accepted {
                    info!(
                        "✅ Streaming offer {} accepted, setting up connection",
                        transfer_id
                    );

                    // Check if peer is still connected
                    if !self.is_peer_connected(peer_id) {
                        warn!(
                            "❌ Peer {} is not connected, cannot start streaming transfer",
                            peer_id
                        );
                        return Ok(());
                    }

                    let mut ft = self.file_transfer.write().await;
                    ft.initiate_streaming_connection(peer_id, transfer_id, suggested_config)
                        .await?;
                } else {
                    warn!(
                        "🚫 Streaming offer {} rejected by peer {}: {:?}",
                        transfer_id, peer_id, reason
                    );
                }
            }

            MessageType::StreamingConnectionRequest { transfer_id, port } => {
                info!(
                    "Received streaming connection request for transfer {}",
                    transfer_id
                );
                self.handle_streaming_connection_request(peer_id, transfer_id, port)
                    .await?;
            }

            MessageType::StreamingConnectionResponse {
                transfer_id,
                accepted,
                port,
                reason,
            } => {
                if accepted {
                    if let Some(port) = port {
                        info!(
                            "Streaming connection approved for transfer {} on port {}",
                            transfer_id, port
                        );
                        self.initiate_streaming_connection(peer_id, transfer_id, port)
                            .await?;
                    }
                } else {
                    warn!(
                        "Streaming connection rejected for transfer {}: {:?}",
                        transfer_id, reason
                    );
                }
            }

            MessageType::StreamingTransferComplete {
                transfer_id,
                bytes_transferred,
                chunks_received,
                file_hash,
            } => {
                info!(
                    "✅ Streaming transfer {} completed: {} bytes, {} chunks",
                    transfer_id, bytes_transferred, chunks_received
                );
                // Handle completion notification
            }

            MessageType::StreamingTransferError {
                transfer_id,
                error,
                chunk_index,
            } => {
                error!(
                    "❌ Streaming transfer {} failed: {} (chunk: {:?})",
                    transfer_id, error, chunk_index
                );
                // Handle error notification
            }

            MessageType::FileRequest {
                request_id,
                file_path,
                target_path,
            } => {
                info!(
                    "Received file request from {}: {} -> {}",
                    peer_id, file_path, target_path
                );
                self.handle_file_request(
                    peer_id,
                    request_id,
                    PathBuf::from(file_path),
                    PathBuf::from(target_path),
                )
                .await?;
            }

            MessageType::FileRequestResponse {
                request_id,
                accepted,
                reason,
            } => {
                if accepted {
                    info!("File request {} accepted by {}", request_id, peer_id);
                } else {
                    warn!(
                        "File request {} rejected by {}: {:?}",
                        request_id, peer_id, reason
                    );
                }
            }

            MessageType::FileOffer {
                transfer_id,
                metadata,
            } => {
                info!(
                    "✅ Processing incoming FileOffer from {}: {}",
                    peer_id, metadata.name
                );

                // Handle the file offer
                let mut ft = self.file_transfer.write().await;
                ft.handle_file_offer(peer_id, transfer_id, metadata).await?;

                // Create and send the response
                let response = ft.create_file_offer_response(transfer_id, true, None);
                drop(ft); // Release the lock

                // Send the response
                if let Some(conn) = self.connections.get(&peer_id) {
                    if let Err(e) = conn.send(response) {
                        error!(
                            "Failed to send FileOfferResponse to peer {}: {}",
                            peer_id, e
                        );
                    } else {
                        info!("✅ Sent FileOfferResponse to peer {}", peer_id);
                    }
                } else {
                    error!(
                        "No connection found for peer {} to send FileOfferResponse",
                        peer_id
                    );
                }
            }

            MessageType::FileOfferResponse {
                transfer_id,
                accepted,
                reason,
            } => {
                info!(
                    "Received file offer response from {}: {}",
                    peer_id, accepted
                );
                let mut ft = self.file_transfer.write().await;
                ft.handle_file_offer_response(peer_id, transfer_id, accepted, reason)
                    .await?;
            }

            MessageType::FileChunk { transfer_id, chunk } => {
                let mut ft = self.file_transfer.write().await;
                ft.handle_file_chunk(peer_id, transfer_id, chunk).await?;
            }

            MessageType::TransferComplete {
                transfer_id,
                checksum,
            } => {
                info!(
                    "✅ Received TransferComplete for transfer {} from peer {}",
                    transfer_id, peer_id
                );

                // Check if we already processed this completion
                let ft = self.file_transfer.read().await;
                if let Some(transfer) = ft.active_transfers.get(&transfer_id) {
                    if matches!(transfer.status, TransferStatus::Completed) {
                        info!(
                            "✅ Transfer {} already marked as complete, ignoring duplicate",
                            transfer_id
                        );
                        return Ok(());
                    }
                }
                drop(ft);

                let mut ft = self.file_transfer.write().await;
                ft.handle_transfer_complete(peer_id, transfer_id, checksum)
                    .await?;
            }

            MessageType::TransferError { transfer_id, error } => {
                error!("Transfer error from {}: {}", peer_id, error);
                let mut ft = self.file_transfer.write().await;
                ft.handle_transfer_error(peer_id, transfer_id, error)
                    .await?;
            }

            MessageType::FileChunkAck {
                transfer_id,
                chunk_index,
            } => {
                debug!(
                    "Received chunk ack for transfer {} chunk {}",
                    transfer_id, chunk_index
                );
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

    async fn handle_streaming_connection_request(
        &mut self,
        peer_id: Uuid,
        transfer_id: Uuid,
        requested_port: u16,
    ) -> Result<()> {
        info!(
            "Setting up streaming connection for transfer {} from peer {}",
            transfer_id, peer_id
        );

        // We can either use the requested port or assign our own
        let port = if requested_port == 0 {
            0 // Let system assign
        } else {
            requested_port
        };

        // For now, accept all streaming connections
        // In production, you might want additional validation

        if let Some(conn) = self.connections.get(&peer_id) {
            let response = Message::new(MessageType::StreamingConnectionResponse {
                transfer_id,
                accepted: true,
                port: Some(self.settings.network.port), // Use our main port for now
                reason: None,
            });

            let _ = conn.send(response);
            info!(
                "✅ Approved streaming connection for transfer {}",
                transfer_id
            );
        }

        Ok(())
    }

    // NEW: Initiate streaming connection
    async fn initiate_streaming_connection(
        &mut self,
        peer_id: Uuid,
        transfer_id: Uuid,
        port: u16,
    ) -> Result<()> {
        info!(
            "Initiating streaming connection to peer {} on port {} for transfer {}",
            peer_id, port, transfer_id
        );

        // Get peer address
        if let Some(peer) = self.peers.get(&peer_id) {
            let mut streaming_addr = peer.device_info.addr;
            streaming_addr.set_port(port);

            // Get connection from pool
            let ft = self.file_transfer.read().await;
            if let Some(connection_pool) = ft.get_connection_pool() {
                match connection_pool
                    .get_connection(peer_id, streaming_addr)
                    .await
                {
                    Ok(stream) => {
                        info!("✅ Got streaming connection for transfer {}", transfer_id);
                        // The actual streaming will be handled by the StreamingTransferManager
                    }
                    Err(e) => {
                        error!("❌ Failed to get streaming connection: {}", e);
                        return Err(e);
                    }
                }
            }
        }

        Ok(())
    }

    async fn handle_file_request(
        &mut self,
        peer_id: Uuid,
        request_id: Uuid,
        file_path: PathBuf,
        target_path: PathBuf,
    ) -> Result<()> {
        info!(
            "Processing file request {} for {:?} (target: {:?})",
            request_id, file_path, target_path
        );

        // Check if the requested file exists
        if !file_path.exists() {
            warn!("Requested file does not exist: {:?}", file_path);
            let response = Message::new(MessageType::FileRequestResponse {
                request_id,
                accepted: false,
                reason: Some("File not found".to_string()),
            });

            if let Some(conn) = self.connections.get(&peer_id) {
                let _ = conn.send(response);
            }
            return Ok(());
        }

        // Accept the request
        let response = Message::new(MessageType::FileRequestResponse {
            request_id,
            accepted: true,
            reason: None,
        });

        if let Some(conn) = self.connections.get(&peer_id) {
            let _ = conn.send(response);
        }

        info!(
            "File request accepted, starting file transfer to peer {} for file: {:?}",
            peer_id, file_path
        );

        // Extract target directory
        let target_dir = Self::extract_target_directory(&target_path);

        info!("Target directory extracted: {:?}", target_dir);

        // Start file transfer with target directory
        let mut ft = self.file_transfer.write().await;
        ft.send_file_with_target_dir(peer_id, file_path, target_dir)
            .await?;

        Ok(())
    }

    fn extract_target_directory(target_path: &PathBuf) -> Option<String> {
        let target_str = target_path.to_string_lossy();
        info!("Extracting directory from target path: '{}'", target_str);

        // Handle both Windows and Unix paths manually for cross-platform compatibility
        let path_separators = ['/', '\\'];

        // Find the last path separator
        if let Some(last_sep_pos) = target_str.rfind(&path_separators[..]) {
            let dir_path = &target_str[..last_sep_pos];
            info!("Extracted directory: '{}'", dir_path);

            if dir_path.is_empty() {
                None
            } else {
                Some(dir_path.to_string())
            }
        } else {
            info!("No path separator found, no directory to extract");
            None
        }
    }
}

// Split PeerConnection into read and write halves
pub struct PeerConnection {
    stream: TcpStream,
}

pub struct PeerConnectionReadHalf {
    stream: tokio::io::ReadHalf<TcpStream>,
}

pub struct PeerConnectionWriteHalf {
    stream: tokio::io::WriteHalf<TcpStream>,
}

impl PeerConnection {
    fn new(stream: TcpStream) -> Self {
        Self { stream }
    }

    fn split(self) -> (PeerConnectionReadHalf, PeerConnectionWriteHalf) {
        let (read_half, write_half) = tokio::io::split(self.stream);
        (
            PeerConnectionReadHalf { stream: read_half },
            PeerConnectionWriteHalf { stream: write_half },
        )
    }

    async fn write_message(&mut self, message: &Message) -> Result<()> {
        // Serialize the message
        let message_data = bincode::serialize(message)?;
        let message_len = message_data.len() as u32;

        // Write length first, then data
        self.stream.write_all(&message_len.to_be_bytes()).await?;
        self.stream.write_all(&message_data).await?;
        self.stream.flush().await?;

        Ok(())
    }

    async fn read_message(&mut self) -> Result<Message> {
        // Read message length first (4 bytes)
        let mut len_bytes = [0u8; 4];
        self.stream.read_exact(&mut len_bytes).await?;
        let message_len = u32::from_be_bytes(len_bytes) as usize;

        // Validate message length to prevent memory attacks
        if message_len > 100_000_000 {
            return Err(FileshareError::Transfer("Message too large".to_string()));
        }

        // Read the message data
        let mut message_data = vec![0u8; message_len];
        self.stream.read_exact(&mut message_data).await?;

        // Deserialize the message
        let message: Message = bincode::deserialize(&message_data)?;

        Ok(message)
    }
}

impl PeerConnectionReadHalf {
    async fn read_message(&mut self) -> Result<Message> {
        // Read message length first (4 bytes)
        let mut len_bytes = [0u8; 4];
        self.stream.read_exact(&mut len_bytes).await?;
        let message_len = u32::from_be_bytes(len_bytes) as usize;

        // Validate message length to prevent memory attacks
        if message_len > 100_000_000 {
            return Err(FileshareError::Transfer("Message too large".to_string()));
        }

        // Read the message data
        let mut message_data = vec![0u8; message_len];
        self.stream.read_exact(&mut message_data).await?;

        // Deserialize the message
        let message: Message = bincode::deserialize(&message_data)?;

        Ok(message)
    }
}

impl PeerConnectionWriteHalf {
    async fn write_message(&mut self, message: &Message) -> Result<()> {
        // Serialize the message
        let message_data = bincode::serialize(message)?;
        let message_len = message_data.len() as u32;

        // Write length first, then data
        self.stream.write_all(&message_len.to_be_bytes()).await?;
        self.stream.write_all(&message_data).await?;
        self.stream.flush().await?;

        Ok(())
    }
}
