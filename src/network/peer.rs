use crate::{
    config::Settings,
    network::{discovery::DeviceInfo, protocol::*},
    service::file_transfer::{FileTransferManager, TransferDirection},
    FileshareError, Result,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct Peer {
    pub device_info: DeviceInfo,
    pub connection_status: ConnectionStatus,
    pub last_ping: Option<std::time::Instant>,
}

#[derive(Debug, Clone)]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
    Authenticated,
    Error(String),
}

pub struct PeerManager {
    settings: Arc<Settings>,
    peers: HashMap<Uuid, Peer>,
    file_transfer: Arc<RwLock<FileTransferManager>>,
    message_tx: mpsc::UnboundedSender<(Uuid, Message)>,
    message_rx: mpsc::UnboundedReceiver<(Uuid, Message)>,
    // Store active connections to send messages
    connections: HashMap<Uuid, mpsc::UnboundedSender<Message>>,
}

impl PeerManager {
    pub async fn debug_message_flow(&self, peer_id: Uuid, message_type: &str, direction: &str) {
        info!("🔍 MESSAGE FLOW DEBUG:");
        info!("  Peer: {}", peer_id);
        info!("  Message: {}", message_type);
        info!("  Direction: {}", direction);
        info!("  Active connections: {}", self.connections.len());

        if let Some(peer) = self.peers.get(&peer_id) {
            info!("  Peer status: {:?}", peer.connection_status);
            info!("  Peer address: {}", peer.device_info.addr);
        }

        for (conn_peer_id, _) in &self.connections {
            info!("  Connected to: {}", conn_peer_id);
        }
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

    pub fn debug_connection_status(&self) {
        info!("=== CONNECTION STATUS DEBUG ===");
        info!("Total discovered peers: {}", self.peers.len());
        info!("Active connections: {}", self.connections.len());

        for (peer_id, peer) in &self.peers {
            info!(
                "Peer {}: {} - Status: {:?}",
                peer_id, peer.device_info.name, peer.connection_status
            );
        }

        for (peer_id, _) in &self.connections {
            info!("Active connection to: {}", peer_id);
        }
        info!("=== END CONNECTION STATUS ===");
    }

    // Update the send_message_to_peer method to add FileOffer-specific debugging
    pub async fn send_message_to_peer(&mut self, peer_id: Uuid, message: Message) -> Result<()> {
        // Clone message type for logging
        let message_type_for_logging = message.message_type.clone();

        self.debug_message_flow(
            peer_id,
            &format!("{:?}", message_type_for_logging),
            "OUTGOING",
        )
        .await;

        // Special logging for FileOffer
        if let MessageType::FileOffer {
            ref transfer_id, ..
        } = message_type_for_logging
        {
            info!(
                "🚀 CRITICAL: Sending FileOffer {} to peer {}",
                transfer_id, peer_id
            );
            info!(
                "🚀 Available connections: {:?}",
                self.connections.keys().collect::<Vec<_>>()
            );
        }

        info!(
            "Attempting to send message to peer {}: {:?}",
            peer_id, message_type_for_logging
        );

        if let Some(conn) = self.connections.get(&peer_id) {
            match conn.send(message) {
                Ok(()) => {
                    info!("✅ Successfully sent message to peer {}", peer_id);
                    if let MessageType::FileOffer {
                        ref transfer_id, ..
                    } = message_type_for_logging
                    {
                        info!(
                            "🚀 FileOffer {} successfully queued for transmission to {}",
                            transfer_id, peer_id
                        );
                    }
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
        };

        info!("Adding new peer: {} ({})", device_info.name, device_info.id);
        self.peers.insert(device_info.id, peer);

        // Log security settings for debugging
        info!(
            "Security settings - require_pairing: {}, allowed_devices: {:?}",
            self.settings.security.require_pairing, self.settings.security.allowed_devices
        );

        // Attempt to connect to new peers only
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

        // For incoming connections, we don't know the peer ID yet
        // We'll discover it during the handshake
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
        let read_peer_id = peer_id; // Capture peer_id for the read task
        let read_task = tokio::spawn(async move {
            loop {
                match read_half.read_message().await {
                    Ok(message) => {
                        // Special logging for FileOffer
                        if let MessageType::FileOffer {
                            ref transfer_id, ..
                        } = message.message_type
                        {
                            info!(
                                "🚀 READ TASK: Received FileOffer {} from peer {}",
                                transfer_id, read_peer_id
                            );
                        }

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
        // Spawn task to handle writing messages TO the peer
        let write_peer_id = peer_id; // Capture peer_id for the write task
        let write_task = tokio::spawn(async move {
            while let Some(message) = conn_rx.recv().await {
                // Clone the message for logging to avoid borrow issues
                let message_type_for_logging = message.message_type.clone();

                // Special logging for FileOffer
                if let MessageType::FileOffer {
                    ref transfer_id, ..
                } = message_type_for_logging
                {
                    info!(
                        "🚀 WRITE TASK: About to transmit FileOffer {} to peer {}",
                        transfer_id, write_peer_id
                    );
                }

                info!(
                    "📤 WRITE to peer {}: {:?}",
                    write_peer_id, message_type_for_logging
                );

                // Send the original message
                if let Err(e) = write_half.write_message(&message).await {
                    error!("Failed to write message to peer {}: {}", write_peer_id, e);
                    break;
                }

                // Confirm FileOffer was sent
                if let MessageType::FileOffer {
                    ref transfer_id, ..
                } = message_type_for_logging
                {
                    info!(
                        "🚀 WRITE TASK: FileOffer {} transmitted to peer {}",
                        transfer_id, write_peer_id
                    );
                }
            }
            info!("Write task ended for peer {}", write_peer_id);
        });

        // Clean up when connection ends
        let cleanup_peer_id = peer_id;
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

        // Start file transfer
        let mut ft = self.file_transfer.write().await;
        ft.send_file(peer_id, file_path).await
    }

    pub fn get_connected_peers(&self) -> Vec<Peer> {
        self.peers
            .values()
            .filter(|peer| matches!(peer.connection_status, ConnectionStatus::Authenticated))
            .cloned()
            .collect()
    }

    pub async fn process_messages(
        &mut self,
        clipboard: &crate::clipboard::ClipboardManager,
    ) -> Result<()> {
        while let Ok((peer_id, message)) = self.message_rx.try_recv() {
            self.handle_message(peer_id, message, clipboard).await?;
        }
        Ok(())
    }

    async fn handle_message(
        &mut self,
        peer_id: Uuid,
        message: Message,
        clipboard: &crate::clipboard::ClipboardManager,
    ) -> Result<()> {
        // Check for FileOffer early but with better logic
        if let MessageType::FileOffer {
            ref transfer_id, ..
        } = message.message_type
        {
            let mut ft = self.file_transfer.write().await;

            // Check if this is our own outgoing transfer that just got sent
            if let Some(direction) = ft.get_transfer_direction(*transfer_id) {
                if matches!(direction, TransferDirection::Outgoing) {
                    info!(
                        "🔄 IGNORING our own outgoing FileOffer {} (this is a loopback)",
                        transfer_id
                    );
                    return Ok(());
                }
            }
            drop(ft); // Release lock

            // If we get here, it's a legitimate incoming FileOffer, so continue processing
            info!(
                "✅ LEGITIMATE incoming FileOffer {} from peer {}",
                transfer_id, peer_id
            );
        }

        // Log all legitimate incoming messages
        self.debug_message_flow(peer_id, &format!("{:?}", message.message_type), "INCOMING")
            .await;

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
                    peer.last_ping = Some(std::time::Instant::now());
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
                // We already validated this is a legitimate incoming offer above
                info!(
                    "✅ Processing incoming FileOffer from {}: {}",
                    peer_id, metadata.name
                );
                let mut ft = self.file_transfer.write().await;
                ft.handle_file_offer(peer_id, transfer_id, metadata).await?;
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

    // Replace the handle_file_request method:
    async fn handle_file_request(
        &mut self,
        peer_id: Uuid,
        request_id: Uuid,
        file_path: PathBuf,
        _target_path: PathBuf,
    ) -> Result<()> {
        info!("Processing file request {} for {:?}", request_id, file_path);

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
            "File request accepted, starting file transfer to peer {}",
            peer_id
        );

        // Use the existing send_file_to_peer method which works correctly
        self.send_file_to_peer(peer_id, file_path).await?;

        Ok(())
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
        // Add logging to track message direction
        info!("📤 SENDING message: {:?} to peer", message.message_type);

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

        // Add logging to track message direction
        info!("📥 RECEIVED message: {:?} from peer", message.message_type);

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

        info!("📥 RECEIVED message: {:?}", message.message_type);

        Ok(message)
    }
}

impl PeerConnectionWriteHalf {
    async fn write_message(&mut self, message: &Message) -> Result<()> {
        info!("📤 SENDING message: {:?}", message.message_type);

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
