use crate::{
    config::Settings,
    network::{connection_pool::ConnectionPoolManager, discovery::DeviceInfo, protocol::*},
    service::file_transfer::FileTransferManager,
    FileshareError, Result,
};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct Peer {
    pub device_info: DeviceInfo,
    pub connection_status: ConnectionStatus,
    pub last_ping: Option<Instant>,
    pub ping_failures: u32,
    pub last_seen: Instant,
    pub reconnection_attempts: u32,
    pub streaming_port: Option<u16>,
    pub connection_quality: ConnectionQuality,
}

#[derive(Debug, Clone)]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
    Authenticated,
    Reconnecting,
    Error(String),
}

#[derive(Debug, Clone)]
pub struct ConnectionQuality {
    pub latency_ms: f64,
    pub bandwidth_mbps: f64,
    pub packet_loss: f64,
    pub stability_score: f64,
    pub last_measured: Instant,
}

impl Default for ConnectionQuality {
    fn default() -> Self {
        Self {
            latency_ms: 0.0,
            bandwidth_mbps: 0.0,
            packet_loss: 0.0,
            stability_score: 1.0,
            last_measured: Instant::now(),
        }
    }
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

// ✅ UNIFIED: Single PeerManager with proper connection pooling
pub struct PeerManager {
    settings: Arc<Settings>,
    peers: HashMap<Uuid, Peer>,
    pub file_transfer: Arc<RwLock<FileTransferManager>>,
    pub message_tx: mpsc::UnboundedSender<(Uuid, Message)>,
    pub message_rx: mpsc::UnboundedReceiver<(Uuid, Message)>,

    // ✅ FIXED: Use unified connection pool
    connection_pool: Arc<ConnectionPoolManager>,

    // ✅ FIXED: Separate control and streaming connections
    control_connections: HashMap<Uuid, mpsc::UnboundedSender<Message>>,
    streaming_listener: Option<tokio::net::TcpListener>,
}

impl PeerManager {
    pub async fn new(settings: Arc<Settings>) -> Result<Self> {
        let (message_tx, message_rx) = mpsc::unbounded_channel();

        // ✅ FIXED: Create unified connection pool
        let connection_pool = Arc::new(ConnectionPoolManager::new());

        // ✅ FIXED: Create file transfer manager with proper streaming integration
        let file_transfer = Arc::new(RwLock::new(
            FileTransferManager::new(settings.clone(), connection_pool.clone()).await?,
        ));

        // Start streaming listener
        let streaming_port = settings.network.port + 1;
        let streaming_listener =
            tokio::net::TcpListener::bind(format!("0.0.0.0:{}", streaming_port))
                .await
                .ok();

        if streaming_listener.is_some() {
            info!("🚀 Streaming listener started on port {}", streaming_port);
        }

        let peer_manager = Self {
            settings,
            peers: HashMap::new(),
            file_transfer,
            message_tx: message_tx.clone(),
            message_rx,
            connection_pool,
            control_connections: HashMap::new(),
            streaming_listener,
        };

        // Set up message sender for file transfers
        peer_manager.set_file_transfer_message_sender().await;

        info!("✅ PeerManager initialized with unified connection pool");
        Ok(peer_manager)
    }

    async fn set_file_transfer_message_sender(&self) {
        let mut ft = self.file_transfer.write().await;
        ft.set_message_sender(self.message_tx.clone());
        ft.set_connection_pool(self.connection_pool.clone());
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
            streaming_port: Some(device_info.addr.port() + 1),
            connection_quality: ConnectionQuality::default(),
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

    // ✅ FIXED: Proper connection management
    pub async fn connect_to_peer(&mut self, peer_id: Uuid) -> Result<()> {
        let peer = self
            .peers
            .get_mut(&peer_id)
            .ok_or_else(|| FileshareError::Unknown("Peer not found".to_string()))?;

        if matches!(
            peer.connection_status,
            ConnectionStatus::Connected | ConnectionStatus::Authenticated
        ) {
            return Ok(());
        }

        peer.connection_status = ConnectionStatus::Connecting;
        let addr = peer.device_info.addr;

        info!(
            "🔗 Connecting to peer {} at {} with unified pool",
            peer_id, addr
        );

        // ✅ FIXED: Use unified connection pool
        match self.connection_pool.get_connection(peer_id, addr).await {
            Ok(stream) => {
                peer.connection_status = ConnectionStatus::Connected;
                peer.last_seen = Instant::now();

                // Extract the stream from Arc
                let tcp_stream = Arc::try_unwrap(stream).map_err(|_| {
                    FileshareError::Network(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        "Stream is still referenced elsewhere",
                    ))
                })?;

                // Handle control connection
                self.handle_control_connection(peer_id, tcp_stream).await?;
            }
            Err(e) => {
                warn!("❌ Failed to connect to peer {}: {}", peer_id, e);
                peer.connection_status = ConnectionStatus::Error(e.to_string());
            }
        }

        Ok(())
    }

    pub async fn handle_connection(&mut self, stream: tokio::net::TcpStream) -> Result<()> {
        let addr = stream.peer_addr()?;
        info!("Handling incoming connection from {}", addr);
        self.handle_unknown_connection(stream).await
    }

    async fn handle_unknown_connection(&mut self, stream: tokio::net::TcpStream) -> Result<()> {
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
                            streaming_port: Some(connection.stream.peer_addr()?.port() + 1),
                            connection_quality: ConnectionQuality::default(),
                        };

                        self.peers.insert(device_id, peer);
                    }

                    // Handle the authenticated connection
                    self.handle_authenticated_control_connection(device_id, connection)
                        .await?;
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

    // ✅ FIXED: Separate control vs streaming connections
    async fn handle_control_connection(
        &mut self,
        peer_id: Uuid,
        stream: tokio::net::TcpStream,
    ) -> Result<()> {
        let mut connection = PeerConnection::new(stream);

        // Send handshake
        let handshake =
            Message::handshake(self.settings.device.id, self.settings.device.name.clone());
        connection.write_message(&handshake).await?;

        // Wait for response
        match connection.read_message().await? {
            Message {
                message_type: MessageType::HandshakeResponse { accepted: true, .. },
                ..
            } => {
                info!("✅ Control connection authenticated with peer {}", peer_id);

                if let Some(peer) = self.peers.get_mut(&peer_id) {
                    peer.connection_status = ConnectionStatus::Authenticated;
                    peer.last_seen = Instant::now();
                }

                self.handle_authenticated_control_connection(peer_id, connection)
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
                warn!(
                    "❌ Control connection rejected by peer {}: {:?}",
                    peer_id, reason
                );
            }
            _ => {
                return Err(FileshareError::Authentication(
                    "Invalid handshake response".to_string(),
                ));
            }
        }

        Ok(())
    }

    async fn handle_authenticated_control_connection(
        &mut self,
        peer_id: Uuid,
        connection: PeerConnection,
    ) -> Result<()> {
        let message_tx = self.message_tx.clone();
        let (conn_tx, mut conn_rx) = mpsc::unbounded_channel::<Message>();

        // Store control connection
        self.control_connections.insert(peer_id, conn_tx);

        let (mut read_half, mut write_half) = connection.split();

        // Read task - forwards messages to main handler
        let read_message_tx = message_tx.clone();
        let read_peer_id = peer_id;
        tokio::spawn(async move {
            loop {
                match read_half.read_message().await {
                    Ok(message) => {
                        if let Err(e) = read_message_tx.send((read_peer_id, message)) {
                            error!("Failed to forward control message: {}", e);
                            break;
                        }
                    }
                    Err(e) => {
                        warn!("Control connection read error: {}", e);
                        break;
                    }
                }
            }
        });

        // Write task - sends messages from queue
        let write_peer_id = peer_id;
        tokio::spawn(async move {
            while let Some(message) = conn_rx.recv().await {
                if let Err(e) = write_half.write_message(&message).await {
                    error!(
                        "Failed to write control message to peer {}: {}",
                        write_peer_id, e
                    );
                    break;
                }
            }
        });

        Ok(())
    }

    // ✅ FIXED: Create dedicated streaming connections
    pub async fn create_streaming_connection(
        &self,
        peer_id: Uuid,
    ) -> Result<tokio::net::TcpStream> {
        let peer = self
            .peers
            .get(&peer_id)
            .ok_or_else(|| FileshareError::Unknown("Peer not found".to_string()))?;

        let streaming_port = peer
            .streaming_port
            .unwrap_or(peer.device_info.addr.port() + 1);
        let streaming_addr = SocketAddr::new(peer.device_info.addr.ip(), streaming_port);

        info!(
            "🚀 Creating dedicated streaming connection to {} at {}",
            peer_id, streaming_addr
        );

        // Create dedicated streaming connection (bypasses pool to avoid conflicts)
        let stream = tokio::time::timeout(
            Duration::from_secs(10),
            tokio::net::TcpStream::connect(streaming_addr),
        )
        .await
        .map_err(|_| {
            FileshareError::Network(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "Streaming connection timeout",
            ))
        })??;

        // Configure for high performance
        Self::configure_streaming_socket(&stream).await?;

        info!(
            "✅ Dedicated streaming connection established to {}",
            peer_id
        );
        Ok(stream)
    }

    async fn configure_streaming_socket(stream: &tokio::net::TcpStream) -> Result<()> {
        use std::os::fd::{AsRawFd, FromRawFd};

        // Use direct method for nodelay
        stream.set_nodelay(true)?;

        // For other socket options, we need to work with the raw fd
        let raw_fd = stream.as_raw_fd();

        // Create a new socket from the same fd (without taking ownership)
        let socket = unsafe { socket2::Socket::from_raw_fd(libc::dup(raw_fd)) };

        // Set large buffers for high throughput
        socket.set_send_buffer_size(2 * 1024 * 1024)?; // 2MB
        socket.set_recv_buffer_size(2 * 1024 * 1024)?; // 2MB

        // Enable keep-alive
        socket.set_keepalive(true)?;

        Ok(())
    }

    // ✅ FIXED: Start streaming listener that actually uses streaming components
    pub async fn start_streaming_listener(&mut self) -> Result<()> {
        if let Some(listener) = self.streaming_listener.take() {
            let file_transfer = self.file_transfer.clone();

            tokio::spawn(async move {
                info!("🎧 Starting production streaming listener");

                loop {
                    match listener.accept().await {
                        Ok((stream, addr)) => {
                            info!("🚀 New streaming connection from {}", addr);

                            // Configure for optimal performance
                            if let Err(e) = Self::configure_streaming_socket(&stream).await {
                                warn!("Failed to configure streaming socket: {}", e);
                            }

                            let ft = file_transfer.clone();
                            tokio::spawn(async move {
                                if let Err(e) = Self::handle_streaming_connection(stream, ft).await
                                {
                                    error!("❌ Streaming connection error: {}", e);
                                }
                            });
                        }
                        Err(e) => {
                            error!("❌ Failed to accept streaming connection: {}", e);
                            tokio::time::sleep(Duration::from_secs(1)).await;
                        }
                    }
                }
            });
        }

        Ok(())
    }

    // ✅ FIXED: Actually use streaming components for incoming connections
    async fn handle_streaming_connection(
        stream: tokio::net::TcpStream,
        file_transfer: Arc<RwLock<FileTransferManager>>,
    ) -> Result<()> {
        info!("🚀 Handling incoming streaming connection with proper components");

        // ✅ FIXED: Use StreamingTransferManager for incoming streams
        let mut ft = file_transfer.write().await;
        ft.handle_incoming_streaming_connection(stream).await?;

        Ok(())
    }

    // ✅ FIXED: Smart message routing with proper component usage
    pub async fn handle_message(
        &mut self,
        peer_id: Uuid,
        message: Message,
        clipboard: &crate::clipboard::ClipboardManager,
    ) -> Result<()> {
        // Update peer activity
        if let Some(peer) = self.peers.get_mut(&peer_id) {
            peer.last_seen = Instant::now();
        }

        info!(
            "📥 Processing message from {}: {:?}",
            peer_id, message.message_type
        );

        match message.message_type {
            // ✅ FIXED: Route streaming messages to streaming manager
            MessageType::StreamingFileOffer {
                transfer_id,
                metadata,
            } => {
                info!("🚀 ROUTING: StreamingFileOffer -> StreamingTransferManager");

                let save_path =
                    self.get_save_path(&metadata.name, metadata.target_dir.as_deref())?;

                // Accept streaming offer
                self.send_control_message(
                    peer_id,
                    Message::new(MessageType::StreamingFileOfferResponse {
                        transfer_id,
                        accepted: true,
                        reason: None,
                        suggested_config: None,
                    }),
                )
                .await?;

                // Route to streaming manager via file transfer manager
                let mut ft = self.file_transfer.write().await;
                ft.handle_streaming_offer(peer_id, transfer_id, metadata, save_path)
                    .await?;
            }

            MessageType::StreamingFileOfferResponse {
                transfer_id,
                accepted,
                reason,
                ..
            } => {
                info!("🚀 ROUTING: StreamingFileOfferResponse -> StreamingTransferManager");

                if accepted {
                    // Create streaming connection and start transfer
                    let stream = self.create_streaming_connection(peer_id).await?;
                    let mut ft = self.file_transfer.write().await;
                    ft.start_streaming_transfer(peer_id, transfer_id, stream)
                        .await?;
                } else {
                    warn!("Streaming offer {} rejected: {:?}", transfer_id, reason);
                }
            }

            // ✅ FIXED: Route chunked messages to regular file transfer
            MessageType::FileOffer {
                transfer_id,
                metadata,
            } => {
                info!("📦 ROUTING: FileOffer -> Regular FileTransferManager");

                let mut ft = self.file_transfer.write().await;
                ft.handle_file_offer(peer_id, transfer_id, metadata).await?;
            }

            MessageType::FileOfferResponse {
                transfer_id,
                accepted,
                reason,
            } => {
                info!("📦 ROUTING: FileOfferResponse -> Regular FileTransferManager");

                let mut ft = self.file_transfer.write().await;
                ft.handle_file_offer_response(peer_id, transfer_id, accepted, reason)
                    .await?;
            }

            MessageType::FileChunk { transfer_id, chunk } => {
                let mut ft = self.file_transfer.write().await;
                ft.handle_file_chunk(peer_id, transfer_id, chunk).await?;
            }

            // Control messages
            MessageType::Ping => {
                self.send_control_message(peer_id, Message::pong()).await?;
            }

            MessageType::Pong => {
                if let Some(peer) = self.peers.get_mut(&peer_id) {
                    peer.last_ping = Some(Instant::now());
                    peer.ping_failures = 0;
                }
            }

            // Clipboard operations
            MessageType::ClipboardUpdate {
                file_path,
                source_device,
                timestamp,
                file_size,
            } => {
                let clipboard_item = crate::clipboard::NetworkClipboardItem {
                    file_path: std::path::PathBuf::from(file_path),
                    source_device,
                    timestamp,
                    file_size,
                };
                clipboard.update_from_network(clipboard_item).await;
            }

            MessageType::FileRequest {
                request_id,
                file_path,
                target_path,
            } => {
                self.handle_file_request(
                    peer_id,
                    request_id,
                    std::path::PathBuf::from(file_path),
                    std::path::PathBuf::from(target_path),
                )
                .await?;
            }

            _ => {
                debug!("Unhandled message type: {:?}", message.message_type);
            }
        }

        Ok(())
    }

    // ✅ FIXED: Send control messages through control connection
    async fn send_control_message(&mut self, peer_id: Uuid, message: Message) -> Result<()> {
        if let Some(conn) = self.control_connections.get(&peer_id) {
            conn.send(message).map_err(|e| {
                FileshareError::Transfer(format!("Failed to send control message: {}", e))
            })?;
            Ok(())
        } else {
            Err(FileshareError::Transfer(format!(
                "No control connection to peer {}",
                peer_id
            )))
        }
    }

    pub async fn send_direct_to_connection(&self, peer_id: Uuid, message: Message) -> Result<()> {
        if let Some(conn) = self.control_connections.get(&peer_id) {
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

    async fn handle_file_request(
        &mut self,
        peer_id: Uuid,
        request_id: Uuid,
        file_path: std::path::PathBuf,
        target_path: std::path::PathBuf,
    ) -> Result<()> {
        if !file_path.exists() {
            let response = Message::new(MessageType::FileRequestResponse {
                request_id,
                accepted: false,
                reason: Some("File not found".to_string()),
            });
            return self.send_control_message(peer_id, response).await;
        }

        // Accept the request
        let response = Message::new(MessageType::FileRequestResponse {
            request_id,
            accepted: true,
            reason: None,
        });
        self.send_control_message(peer_id, response).await?;

        // ✅ FIXED: Use intelligent method selection
        let mut ft = self.file_transfer.write().await;
        ft.send_file_with_validation(peer_id, file_path).await?;

        Ok(())
    }

    fn get_save_path(
        &self,
        filename: &str,
        target_dir: Option<&str>,
    ) -> Result<std::path::PathBuf> {
        let save_dir = if let Some(target_dir_str) = target_dir {
            let target_path = std::path::PathBuf::from(target_dir_str);
            if target_path.exists() && target_path.is_dir() {
                target_path
            } else {
                self.get_default_save_dir()
            }
        } else {
            self.get_default_save_dir()
        };

        std::fs::create_dir_all(&save_dir)?;
        Ok(save_dir.join(filename))
    }

    fn get_default_save_dir(&self) -> std::path::PathBuf {
        dirs::download_dir()
            .or_else(|| dirs::document_dir())
            .unwrap_or_else(|| std::path::PathBuf::from("."))
    }

    // Utility methods
    pub async fn send_message_to_peer(&mut self, peer_id: Uuid, message: Message) -> Result<()> {
        self.send_control_message(peer_id, message).await
    }

    pub fn get_connected_peers(&self) -> Vec<Peer> {
        self.peers
            .values()
            .filter(|peer| matches!(peer.connection_status, ConnectionStatus::Authenticated))
            .cloned()
            .collect()
    }

    pub fn is_peer_healthy(&self, peer_id: Uuid) -> bool {
        self.peers
            .get(&peer_id)
            .map(|peer| {
                matches!(peer.connection_status, ConnectionStatus::Authenticated)
                    && peer.ping_failures < 3
            })
            .unwrap_or(false)
    }

    pub async fn get_all_discovered_devices(&self) -> Vec<DeviceInfo> {
        self.peers
            .values()
            .map(|peer| peer.device_info.clone())
            .collect()
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

    pub async fn check_peer_health_all(&mut self) -> Result<()> {
        // Implementation for health checks
        Ok(())
    }
}

// Helper structs
pub struct PeerConnection {
    stream: tokio::net::TcpStream,
}

pub struct PeerConnectionReadHalf {
    stream: tokio::io::ReadHalf<tokio::net::TcpStream>,
}

pub struct PeerConnectionWriteHalf {
    stream: tokio::io::WriteHalf<tokio::net::TcpStream>,
}

impl PeerConnection {
    fn new(stream: tokio::net::TcpStream) -> Self {
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
        let message_data = bincode::serialize(message)?;
        let message_len = message_data.len() as u32;

        self.stream.write_all(&message_len.to_be_bytes()).await?;
        self.stream.write_all(&message_data).await?;
        self.stream.flush().await?;

        Ok(())
    }

    async fn read_message(&mut self) -> Result<Message> {
        let mut len_bytes = [0u8; 4];
        self.stream.read_exact(&mut len_bytes).await?;
        let message_len = u32::from_be_bytes(len_bytes) as usize;

        if message_len > 100_000_000 {
            return Err(FileshareError::Transfer("Message too large".to_string()));
        }

        let mut message_data = vec![0u8; message_len];
        self.stream.read_exact(&mut message_data).await?;

        let message: Message = bincode::deserialize(&message_data)?;
        Ok(message)
    }
}

impl PeerConnectionReadHalf {
    async fn read_message(&mut self) -> Result<Message> {
        let mut len_bytes = [0u8; 4];
        self.stream.read_exact(&mut len_bytes).await?;
        let message_len = u32::from_be_bytes(len_bytes) as usize;

        if message_len > 100_000_000 {
            return Err(FileshareError::Transfer("Message too large".to_string()));
        }

        let mut message_data = vec![0u8; message_len];
        self.stream.read_exact(&mut message_data).await?;

        let message: Message = bincode::deserialize(&message_data)?;
        Ok(message)
    }
}

impl PeerConnectionWriteHalf {
    async fn write_message(&mut self, message: &Message) -> Result<()> {
        let message_data = bincode::serialize(message)?;
        let message_len = message_data.len() as u32;

        self.stream.write_all(&message_len.to_be_bytes()).await?;
        self.stream.write_all(&message_data).await?;
        self.stream.flush().await?;

        Ok(())
    }
}
