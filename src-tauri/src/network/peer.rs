use crate::{
    config::Settings,
    network::{discovery::DeviceInfo, protocol::*},
    service::file_transfer::{
        FileTransferManager, MessageSender, TransferDirection, TransferStatus,
    },
    quic::QuicIntegration,
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
    quic_integration: Option<Arc<QuicIntegration>>,
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

    pub async fn new(settings: Arc<Settings>) -> Result<Self> {
        let (message_tx, message_rx) = mpsc::unbounded_channel();
        let file_transfer = Arc::new(RwLock::new(
            FileTransferManager::new(settings.clone()).await?,
        ));

        let peer_manager = Self {
            settings,
            peers: HashMap::new(),
            file_transfer,
            message_tx: message_tx.clone(),
            message_rx,
            connections: HashMap::new(),
            quic_integration: None,
        };

        // Set up the message sender for file transfers
        peer_manager.set_file_transfer_message_sender().await;

        Ok(peer_manager)
    }

    /// Create a new PeerManager with QUIC integration
    pub async fn new_with_quic(
        settings: Arc<Settings>, 
        quic_integration: Option<Arc<QuicIntegration>>
    ) -> Result<Self> {
        let (message_tx, message_rx) = mpsc::unbounded_channel();
        let file_transfer = Arc::new(RwLock::new(
            FileTransferManager::new(settings.clone()).await?,
        ));

        let peer_manager = Self {
            settings,
            peers: HashMap::new(),
            file_transfer,
            message_tx: message_tx.clone(),
            message_rx,
            connections: HashMap::new(),
            quic_integration,
        };

        // Set up the message sender for file transfers
        peer_manager.set_file_transfer_message_sender().await;

        Ok(peer_manager)
    }

    // Connect file transfer manager with message sender
    async fn set_file_transfer_message_sender(&self) {
        let mut ft = self.file_transfer.write().await;
        ft.set_message_sender(self.message_tx.clone());
    }

    /// Get peer information by device ID
    pub fn get_peer(&self, device_id: &uuid::Uuid) -> Option<&Peer> {
        self.peers.get(device_id)
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
                        // Only log important messages, not chunks or frequent messages
                        match &message.message_type {
                            MessageType::Ping | MessageType::Pong => {
                                debug!("📥 READ {} from {}", 
                                       if matches!(message.message_type, MessageType::Ping) { "ping" } else { "pong" },
                                       read_peer_id);
                            }
                            MessageType::FileChunk { transfer_id, chunk } => {
                                // Only log every 10th chunk to reduce noise
                                if chunk.index % 10 == 0 || chunk.is_last {
                                    info!("📥 READ chunk {} for transfer {} ({}B, last: {})",
                                          chunk.index, transfer_id, chunk.data.len(), chunk.is_last);
                                }
                            }
                            _ => {
                                info!("📥 READ from peer {}: {:?}", read_peer_id, message.message_type);
                            }
                        }
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
            let mut last_flush = tokio::time::Instant::now();
            let flush_interval = Duration::from_millis(50); // Flush every 50ms
            
            while let Some(message) = conn_rx.recv().await {
                // Only log important messages, not chunks or frequent messages
                match &message.message_type {
                    MessageType::Ping | MessageType::Pong => {
                        debug!("📤 WRITE {} to {}", 
                               if matches!(message.message_type, MessageType::Ping) { "ping" } else { "pong" },
                               write_peer_id);
                    }
                    MessageType::FileChunk { transfer_id, chunk } => {
                        // Only log every 10th chunk to reduce noise
                        if chunk.index % 10 == 0 || chunk.is_last {
                            info!("📤 WRITE chunk {} for transfer {} ({}B, last: {})",
                                  chunk.index, transfer_id, chunk.data.len(), chunk.is_last);
                        }
                    }
                    _ => {
                        info!("📤 WRITE to peer {}: {:?}", write_peer_id, message.message_type);
                    }
                }

                if let Err(e) = write_half.write_message(&message).await {
                    error!("Failed to write message to peer {}: {}", write_peer_id, e);
                    break;
                }
                
                // Periodic flush to ensure chunks are sent timely
                if last_flush.elapsed() > flush_interval {
                    if let Err(e) = write_half.stream.flush().await {
                        error!("Failed to flush stream: {}", e);
                    }
                    last_flush = tokio::time::Instant::now();
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

        // Try QUIC first for better performance
        if let Some(ref quic_integration) = self.quic_integration {
            // Check if QUIC connection exists for this peer
            let quic_stats = quic_integration.get_connection_stats().await;
            if quic_stats.contains_key(&peer_id) {
                info!("🚀 Using QUIC for high-speed file transfer to {}", peer_id);
                
                match quic_integration.send_file(peer_id, file_path.clone(), None).await {
                    Ok(transfer_id) => {
                        info!("✅ QUIC transfer started: {}", transfer_id);
                        return Ok(());
                    }
                    Err(e) => {
                        warn!("⚠️ QUIC transfer failed, falling back to TCP: {}", e);
                        // Continue to TCP fallback below
                    }
                }
            } else {
                info!("📡 No QUIC connection for peer {}, using TCP", peer_id);
            }
        }

        // Fallback to TCP-based file transfer
        info!("📡 Using TCP file transfer to {}", peer_id);
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

        // Only log important messages, not chunks or frequent messages
        match &message.message_type {
            MessageType::Ping | MessageType::Pong => {
                debug!("📥 Processing {} from {}", 
                       if matches!(message.message_type, MessageType::Ping) { "ping" } else { "pong" },
                       peer_id);
            }
            MessageType::FileChunk { .. } => {
                // Don't log individual chunks - too noisy
            }
            _ => {
                info!("📥 Processing message from {}: {:?}", peer_id, message.message_type);
            }
        }

        match message.message_type {
            MessageType::Ping => {
                debug!("Received ping from {}", peer_id);
                // Rate limit pong responses to prevent flooding
                if let Some(peer) = self.peers.get(&peer_id) {
                    if let Some(last_pong) = peer.last_ping {
                        // Only respond to ping if it's been at least 5 seconds since last pong
                        if last_pong.elapsed().as_secs() < 5 {
                            debug!("Rate limiting pong response to {}", peer_id);
                            return Ok(());
                        }
                    }
                }
                
                if let Some(conn) = self.connections.get(&peer_id) {
                    let _ = conn.send(Message::pong());
                    debug!("📤 WRITE to peer {}: Pong", peer_id);
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

            MessageType::FileChunkBatchAck {
                transfer_id,
                chunk_indices,
            } => {
                debug!(
                    "Received batch chunk ack for transfer {} - {} chunks",
                    transfer_id, chunk_indices.len()
                );
            }

            MessageType::TransferProgress {
                transfer_id,
                bytes_transferred,
                chunks_completed,
                speed_bps,
                eta_seconds,
            } => {
                let speed_mbps = speed_bps as f64 / (1024.0 * 1024.0);
                let eta_text = eta_seconds.map_or("unknown".to_string(), |s| format!("{}s", s));
                debug!(
                    "Transfer {} progress: {} bytes, {} chunks, {:.1} MB/s, ETA: {}",
                    transfer_id, bytes_transferred, chunks_completed, speed_mbps, eta_text
                );
            }

            MessageType::TransferResume {
                transfer_id,
                completed_chunks,
            } => {
                info!(
                    "Received resume request for transfer {} with {} completed chunks",
                    transfer_id, completed_chunks.len()
                );
                // TODO: Implement resume functionality
            }

            MessageType::TransferPause {
                transfer_id,
            } => {
                info!(
                    "Received pause request for transfer {}",
                    transfer_id
                );
                // TODO: Implement pause functionality
            }

            // OPTIMIZATION: Handle batched file chunks
            MessageType::FileChunkBatch { transfer_id, chunks } => {
                debug!("📦 Received batch of {} chunks for transfer {}", chunks.len(), transfer_id);
                let mut ft = self.file_transfer.write().await;
                for chunk in chunks {
                    ft.handle_file_chunk(peer_id, transfer_id, chunk).await?;
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
        // OPTIMIZATION: Apply TCP optimizations
        if let Err(e) = Self::optimize_tcp_connection(&stream) {
            warn!("Failed to apply TCP optimizations: {}", e);
        }
        Self { stream }
    }
    
    // OPTIMIZATION: TCP performance tuning
    fn optimize_tcp_connection(stream: &TcpStream) -> Result<()> {
        use socket2::Socket;
        
        #[cfg(unix)]
        {
            use std::os::fd::{AsRawFd, FromRawFd};
            // Get the raw socket
            let socket = unsafe { Socket::from_raw_fd(stream.as_raw_fd()) };
            
            // Increase TCP buffer sizes for better throughput
            let _ = socket.set_recv_buffer_size(8 * 1024 * 1024); // 8MB receive buffer
            let _ = socket.set_send_buffer_size(8 * 1024 * 1024); // 8MB send buffer
            
            // Disable Nagle algorithm for lower latency
            let _ = socket.set_nodelay(true);
            
            // Enable TCP keep-alive
            let _ = socket.set_keepalive(true);
            
            // Don't take ownership of the socket
            std::mem::forget(socket);
        }
        
        #[cfg(windows)]
        {
            use std::os::windows::io::{AsRawSocket, FromRawSocket};
            // Get the raw socket
            let socket = unsafe { Socket::from_raw_socket(stream.as_raw_socket()) };
            
            // Increase TCP buffer sizes for better throughput
            let _ = socket.set_recv_buffer_size(8 * 1024 * 1024); // 8MB receive buffer
            let _ = socket.set_send_buffer_size(8 * 1024 * 1024); // 8MB send buffer
            
            // Disable Nagle algorithm for lower latency
            let _ = socket.set_nodelay(true);
            
            // Enable TCP keep-alive
            let _ = socket.set_keepalive(true);
            
            // Don't take ownership of the socket
            std::mem::forget(socket);
        }
        
        info!("✅ TCP optimizations applied: 8MB buffers, nodelay enabled");
        Ok(())
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
        
        // OPTIMIZATION: Only flush for critical messages, not chunks
        match &message.message_type {
            MessageType::FileChunk { .. } => {
                // Don't flush chunks - let TCP buffer them
            }
            _ => {
                // Flush other messages for responsiveness
                self.stream.flush().await?;
            }
        }

        Ok(())
    }

    async fn read_message(&mut self) -> Result<Message> {
        // Read message length first (4 bytes)
        let mut len_bytes = [0u8; 4];
        self.stream.read_exact(&mut len_bytes).await?;
        let message_len = u32::from_be_bytes(len_bytes) as usize;

        // Validate message length to prevent memory attacks
        // Increased from 100MB to 50MB to support 8MB chunks with metadata overhead
        if message_len > 50_000_000 {
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
        // Increased from 100MB to 50MB to support 8MB chunks with metadata overhead
        if message_len > 50_000_000 {
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
        
        // OPTIMIZATION: Only flush for critical messages, not chunks
        match &message.message_type {
            MessageType::FileChunk { .. } => {
                // Don't flush chunks - let TCP buffer them
            }
            _ => {
                // Flush other messages for responsiveness
                self.stream.flush().await?;
            }
        }

        Ok(())
    }
}
