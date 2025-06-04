use crate::{
    config::Settings,
    network::{discovery::DeviceInfo, protocol::*},
    service::file_transfer::FileTransferManager,
    FileshareError, Result,
};
use std::collections::HashMap;
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
}

impl PeerManager {
    pub async fn new(settings: Arc<Settings>) -> Result<Self> {
        let (message_tx, message_rx) = mpsc::unbounded_channel();
        let file_transfer = Arc::new(RwLock::new(
            FileTransferManager::new(settings.clone()).await?,
        ));

        Ok(Self {
            settings,
            peers: HashMap::new(),
            file_transfer,
            message_tx,
            message_rx,
        })
    }

    pub async fn on_device_discovered(&mut self, device_info: DeviceInfo) -> Result<()> {
        let peer = Peer {
            device_info: device_info.clone(),
            connection_status: ConnectionStatus::Disconnected,
            last_ping: None,
        };

        info!("Adding peer: {} ({})", device_info.name, device_info.id);
        self.peers.insert(device_info.id, peer);

        // Optionally, attempt to connect immediately
        if self.settings.security.require_pairing {
            if self
                .settings
                .security
                .allowed_devices
                .contains(&device_info.id)
            {
                self.connect_to_peer(device_info.id).await?;
            }
        } else {
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
            return Ok(());
        }

        peer.connection_status = ConnectionStatus::Connecting;
        let addr = peer.device_info.addr;

        info!("Connecting to peer {} at {}", peer_id, addr);

        match TcpStream::connect(addr).await {
            Ok(stream) => {
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
        mut connection: PeerConnection,
    ) -> Result<()> {
        info!("Handling authenticated connection with peer {}", peer_id);

        let message_tx = self.message_tx.clone();

        // Spawn a task to handle this connection
        tokio::spawn(async move {
            loop {
                match connection.read_message().await {
                    Ok(message) => {
                        debug!(
                            "Received message from {}: {:?}",
                            peer_id, message.message_type
                        );
                        if let Err(e) = message_tx.send((peer_id, message)) {
                            error!("Failed to forward message: {}", e);
                            break;
                        }
                    }
                    Err(e) => {
                        warn!("Connection error with peer {}: {}", peer_id, e);
                        break;
                    }
                }
            }

            info!("Connection handler for peer {} ended", peer_id);
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
        // Changed from Vec<&Peer> to Vec<Peer>
        self.peers
            .values()
            .filter(|peer| matches!(peer.connection_status, ConnectionStatus::Authenticated))
            .cloned() // Clone the peers instead of returning references
            .collect()
    }

    pub async fn process_messages(&mut self) -> Result<()> {
        while let Ok((peer_id, message)) = self.message_rx.try_recv() {
            self.handle_message(peer_id, message).await?;
        }
        Ok(())
    }

    async fn handle_message(&mut self, peer_id: Uuid, message: Message) -> Result<()> {
        match message.message_type {
            MessageType::Ping => {
                debug!("Received ping from {}", peer_id);
                // TODO: Send pong response
            }
            MessageType::Pong => {
                debug!("Received pong from {}", peer_id);
                if let Some(peer) = self.peers.get_mut(&peer_id) {
                    peer.last_ping = Some(std::time::Instant::now());
                }
            }
            MessageType::FileOffer {
                transfer_id,
                metadata,
            } => {
                info!("Received file offer from {}: {}", peer_id, metadata.name);
                let mut ft = self.file_transfer.write().await;
                ft.handle_file_offer(peer_id, transfer_id, metadata).await?;
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

struct PeerConnection {
    stream: TcpStream,
}

impl PeerConnection {
    fn new(stream: TcpStream) -> Self {
        Self { stream }
    }

    async fn read_message(&mut self) -> Result<Message> {
        // Read message length first (4 bytes)
        let mut len_bytes = [0u8; 4];
        self.stream.read_exact(&mut len_bytes).await?;
        let message_len = u32::from_be_bytes(len_bytes) as usize;

        // Read the message data
        let mut message_data = vec![0u8; message_len];
        self.stream.read_exact(&mut message_data).await?;

        // Deserialize the message
        let message: Message = bincode::deserialize(&message_data)?;
        Ok(message)
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
}
