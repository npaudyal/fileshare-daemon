use crate::quic::protocol::*;
use crate::Result;
use quinn::{ClientConfig, Connection, Endpoint, RecvStream, SendStream, ServerConfig};
use rustls::{Certificate, PrivateKey};
use quinn::VarInt;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Mutex, RwLock};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// High-performance QUIC connection manager for file transfers
/// Manages multiple parallel streams per connection for maximum throughput
pub struct QuicConnectionManager {
    endpoint: Endpoint,
    connections: Arc<RwLock<HashMap<Uuid, QuicPeerConnection>>>,
    device_id: Uuid,
    device_name: String,
    capabilities: TransferCapabilities,
    message_tx: mpsc::UnboundedSender<(Uuid, QuicMessage)>,
    message_rx: Arc<Mutex<mpsc::UnboundedReceiver<(Uuid, QuicMessage)>>>,
}

#[derive(Debug)]
pub struct QuicPeerConnection {
    pub peer_id: Uuid,
    pub connection: Connection,
    pub control_send: Option<SendStream>,
    pub control_recv: Option<RecvStream>,
    pub data_streams: HashMap<u8, (SendStream, RecvStream)>,
    pub progress_send: Option<SendStream>,
    pub progress_recv: Option<RecvStream>,
    pub last_activity: Instant,
    pub capabilities: Option<TransferCapabilities>,
    pub status: ConnectionStatus,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConnectionStatus {
    Connecting,
    Connected,
    Authenticated,
    Error(String),
    Closed,
}

impl QuicConnectionManager {
    pub async fn new(bind_addr: SocketAddr, device_id: Uuid, device_name: String) -> Result<Self> {
        // Generate self-signed certificate for QUIC
        let cert = generate_self_signed_cert()?;

        // Create server config
        let server_config = configure_server(cert.clone())?;

        // Create client config (allow self-signed certs for P2P)
        let client_config = configure_client()?;

        // Create endpoint
        let mut endpoint = Endpoint::server(server_config, bind_addr)?;
        endpoint.set_default_client_config(client_config);

        info!("🚀 QUIC endpoint created on {}", bind_addr);

        let (message_tx, message_rx) = mpsc::unbounded_channel();

        Ok(Self {
            endpoint,
            connections: Arc::new(RwLock::new(HashMap::new())),
            device_id,
            device_name,
            capabilities: TransferCapabilities::default(),
            message_tx,
            message_rx: Arc::new(Mutex::new(message_rx)),
        })
    }

    /// Start the connection manager and listen for incoming connections
    pub async fn start(&self) -> Result<()> {
        let endpoint = self.endpoint.clone();
        let connections = self.connections.clone();
        let device_id = self.device_id;
        let device_name = self.device_name.clone();
        let capabilities = self.capabilities.clone();
        let message_tx = self.message_tx.clone();

        // Spawn task to handle incoming connections
        tokio::spawn(async move {
            while let Some(connecting) = endpoint.accept().await {
                let peer_addr = connecting.remote_address();
                info!("📥 Incoming QUIC connection from {}", peer_addr);

                let connections = connections.clone();
                let device_id = device_id;
                let device_name = device_name.clone();
                let capabilities = capabilities.clone();
                let message_tx = message_tx.clone();

                tokio::spawn(async move {
                    match connecting.await {
                        Ok(connection) => {
                            info!("✅ QUIC connection established with {}", peer_addr);

                            if let Err(e) = Self::handle_new_connection(
                                connection,
                                connections,
                                device_id,
                                device_name,
                                capabilities,
                                message_tx,
                                true, // is_incoming
                            )
                            .await
                            {
                                error!("❌ Failed to handle connection from {}: {}", peer_addr, e);
                            }
                        }
                        Err(e) => {
                            error!("❌ Failed to accept connection from {}: {}", peer_addr, e);
                        }
                    }
                });
            }
        });

        info!("✅ QUIC connection manager started");
        Ok(())
    }

    /// Connect to a peer
    pub async fn connect_to_peer(&self, peer_addr: SocketAddr, peer_id: Uuid) -> Result<()> {
        info!("🔗 Connecting to peer {} at {}", peer_id, peer_addr);

        let connecting = self.endpoint.connect(peer_addr, "localhost")
            .map_err(|e| crate::FileshareError::Transfer(format!("QUIC connect error: {}", e)))?;
        let connection = connecting.await
            .map_err(|e| crate::FileshareError::Transfer(format!("QUIC connection error: {}", e)))?;

        info!("✅ QUIC connection established with {}", peer_addr);

        Self::handle_new_connection(
            connection,
            self.connections.clone(),
            self.device_id,
            self.device_name.clone(),
            self.capabilities.clone(),
            self.message_tx.clone(),
            false, // is_incoming
        )
        .await?;

        Ok(())
    }

    async fn handle_new_connection(
        connection: Connection,
        connections: Arc<RwLock<HashMap<Uuid, QuicPeerConnection>>>,
        device_id: Uuid,
        device_name: String,
        capabilities: TransferCapabilities,
        message_tx: mpsc::UnboundedSender<(Uuid, QuicMessage)>,
        is_incoming: bool,
    ) -> Result<()> {
        // Open control stream
        let (control_send, control_recv) = if is_incoming {
            // Wait for incoming control stream
            let (send, recv) = connection.accept_bi().await
                .map_err(|e| crate::FileshareError::Transfer(format!("Accept bi stream error: {}", e)))?;
            (Some(send), Some(recv))
        } else {
            // Open outgoing control stream
            let (send, recv) = connection.open_bi().await
                .map_err(|e| crate::FileshareError::Transfer(format!("Open bi stream error: {}", e)))?;
            (Some(send), Some(recv))
        };

        // Create peer connection
        let peer_connection = QuicPeerConnection {
            peer_id: Uuid::new_v4(), // Will be updated after handshake
            connection: connection.clone(),
            control_send,
            control_recv,
            data_streams: HashMap::new(),
            progress_send: None,
            progress_recv: None,
            last_activity: Instant::now(),
            capabilities: None,
            status: ConnectionStatus::Connected,
        };

        // Store connection temporarily (will update with real peer_id after handshake)
        let temp_id = Uuid::new_v4();
        {
            let mut conns = connections.write().await;
            conns.insert(temp_id, peer_connection);
        }

        // Start handshake
        if !is_incoming {
            let handshake = QuicMessage::Handshake {
                device_id,
                device_name,
                version: env!("CARGO_PKG_VERSION").to_string(),
                capabilities,
            };

            // Send handshake on control stream
            Self::send_control_message(&connection, &handshake).await?;
        }

        // Start message processing for this connection
        Self::process_connection_messages(connection, connections, message_tx, temp_id).await;

        Ok(())
    }

    async fn send_control_message(connection: &Connection, message: &QuicMessage) -> Result<()> {
        let mut control_stream = connection.open_uni().await
            .map_err(|e| crate::FileshareError::Transfer(format!("Open uni stream error: {}", e)))?;
        let data = bincode::serialize(message)?;
        
        use tokio::io::AsyncWriteExt;
        control_stream.write_all(&data).await
            .map_err(|e| crate::FileshareError::Transfer(format!("Write error: {}", e)))?;
        control_stream.shutdown().await
            .map_err(|e| crate::FileshareError::Transfer(format!("Shutdown error: {}", e)))?;
        Ok(())
    }

    async fn process_connection_messages(
        connection: Connection,
        connections: Arc<RwLock<HashMap<Uuid, QuicPeerConnection>>>,
        message_tx: mpsc::UnboundedSender<(Uuid, QuicMessage)>,
        temp_id: Uuid,
    ) {
        loop {
            tokio::select! {
                // Handle incoming streams
                stream_result = connection.accept_uni() => {
                    match stream_result {
                        Ok(mut recv_stream) => {
                            let message_tx_clone = message_tx.clone();
                            tokio::spawn(async move {
                                match recv_stream.read_to_end(256 * 1024).await {
                                    Ok(buffer) => {
                                        // Try to deserialize message
                                        match bincode::deserialize::<QuicMessage>(&buffer) {
                                            Ok(message) => {
                                                if let Err(e) = message_tx_clone.send((temp_id, message)) {
                                                    error!("❌ Failed to send message to processor: {}", e);
                                                }
                                            }
                                            Err(e) => {
                                                error!("❌ Failed to deserialize message: {}", e);
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        error!("❌ Failed to read from stream: {}", e);
                                    }
                                }
                            });
                        }
                        Err(e) => {
                            error!("❌ Connection error: {}", e);
                            break;
                        }
                    }
                }

                // Handle bidirectional streams for data transfer
                bi_stream_result = connection.accept_bi() => {
                    match bi_stream_result {
                        Ok((send, recv)) => {
                            // Handle data streams here
                            debug!("📊 Accepted bidirectional stream");
                            // TODO: Implement data stream handling
                        }
                        Err(e) => {
                            error!("❌ Bidirectional stream error: {}", e);
                        }
                    }
                }
            }
        }

        // Clean up connection
        {
            let mut conns = connections.write().await;
            conns.remove(&temp_id);
        }
        info!("🧹 Connection {} cleaned up", temp_id);
    }

    /// Send a message to a specific peer
    pub async fn send_message(&self, peer_id: Uuid, message: QuicMessage) -> Result<()> {
        let connections = self.connections.read().await;
        if let Some(peer_conn) = connections.get(&peer_id) {
            Self::send_control_message(&peer_conn.connection, &message).await?;
            Ok(())
        } else {
            Err(crate::FileshareError::Transfer(format!(
                "Peer {} not found",
                peer_id
            )))
        }
    }

    /// Get message receiver for processing
    pub fn get_message_receiver(&self) -> Arc<Mutex<mpsc::UnboundedReceiver<(Uuid, QuicMessage)>>> {
        self.message_rx.clone()
    }

    /// Open data streams for file transfer
    pub async fn open_data_streams(&self, peer_id: Uuid, stream_count: u8) -> Result<()> {
        let mut connections = self.connections.write().await;
        if let Some(peer_conn) = connections.get_mut(&peer_id) {
            for i in 1..=stream_count {
                let (send, recv) = peer_conn.connection.open_bi().await
                    .map_err(|e| crate::FileshareError::Transfer(format!("Open data stream error: {}", e)))?;
                peer_conn.data_streams.insert(i, (send, recv));
                debug!("📊 Opened data stream {} for peer {}", i, peer_id);
            }
            info!(
                "✅ Opened {} data streams for peer {}",
                stream_count, peer_id
            );
            Ok(())
        } else {
            Err(crate::FileshareError::Transfer(format!(
                "Peer {} not found",
                peer_id
            )))
        }
    }

    /// Get connection statistics
    pub async fn get_connection_stats(&self) -> HashMap<Uuid, ConnectionStats> {
        let connections = self.connections.read().await;
        let mut stats = HashMap::new();

        for (peer_id, conn) in connections.iter() {
            let quinn_stats = conn.connection.stats();
            stats.insert(
                *peer_id,
                ConnectionStats {
                    bytes_sent: quinn_stats.udp_tx.bytes,
                    bytes_received: quinn_stats.udp_rx.bytes,
                    packets_sent: quinn_stats.udp_tx.datagrams,
                    packets_received: quinn_stats.udp_rx.datagrams,
                    rtt: quinn_stats.path.rtt,
                    congestion_window: quinn_stats.path.cwnd,
                    data_streams: conn.data_streams.len() as u8,
                },
            );
        }

        stats
    }
}

#[derive(Debug, Clone)]
pub struct ConnectionStats {
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub packets_sent: u64,
    pub packets_received: u64,
    pub rtt: Duration,
    pub congestion_window: u64,
    pub data_streams: u8,
}

// Helper functions for QUIC configuration
fn generate_self_signed_cert() -> Result<(Certificate, PrivateKey)> {
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).map_err(|e| {
        crate::FileshareError::Transfer(format!("Certificate generation error: {}", e))
    })?;
    let cert_der = Certificate(cert.serialize_der().map_err(|e| {
        crate::FileshareError::Transfer(format!("Certificate serialization error: {}", e))
    })?);
    let private_key_der = PrivateKey(cert.serialize_private_key_der());
    Ok((cert_der, private_key_der))
}

fn configure_server((cert, key): (Certificate, PrivateKey)) -> Result<ServerConfig> {
    let cert_chain = vec![cert];

    let mut server_config = ServerConfig::with_single_cert(cert_chain, key)
        .map_err(|e| crate::FileshareError::Transfer(format!("TLS config error: {}", e)))?;

    // Configure transport for high throughput
    server_config.transport_config(Arc::new({
        let mut transport = quinn::TransportConfig::default();
        transport.max_concurrent_uni_streams(256_u32.into());
        transport.max_concurrent_bidi_streams(64_u32.into());
        transport.send_window(8 * 1024 * 1024); // 8MB send window
        transport.receive_window(VarInt::from_u32(8 * 1024 * 1024)); // 8MB receive window
        transport.stream_receive_window(VarInt::from_u32(2 * 1024 * 1024)); // 2MB per stream
        transport.datagram_receive_buffer_size(Some(16 * 1024 * 1024)); // 16MB datagram buffer
        transport
    }));

    Ok(server_config)
}

fn configure_client() -> Result<ClientConfig> {
    let crypto = rustls::ClientConfig::builder()
        .with_safe_defaults()
        .with_custom_certificate_verifier(Arc::new(SkipServerVerification {}))
        .with_no_client_auth();

    let mut client_config = ClientConfig::new(Arc::new(crypto));

    // Configure transport for high throughput
    client_config.transport_config(Arc::new({
        let mut transport = quinn::TransportConfig::default();
        transport.max_concurrent_uni_streams(256_u32.into());
        transport.max_concurrent_bidi_streams(64_u32.into());
        transport.send_window(8 * 1024 * 1024); // 8MB send window
        transport.receive_window(VarInt::from_u32(8 * 1024 * 1024)); // 8MB receive window
        transport.stream_receive_window(VarInt::from_u32(2 * 1024 * 1024)); // 2MB per stream
        transport.datagram_receive_buffer_size(Some(16 * 1024 * 1024)); // 16MB datagram buffer
        transport
    }));

    Ok(client_config)
}

// Skip certificate verification for P2P connections
#[derive(Debug)]
struct SkipServerVerification;

impl rustls::client::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::Certificate,
        _intermediates: &[rustls::Certificate],
        _server_name: &rustls::ServerName,
        _scts: &mut dyn Iterator<Item = &[u8]>,
        _ocsp_response: &[u8],
        _now: std::time::SystemTime,
    ) -> std::result::Result<rustls::client::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::ServerCertVerified::assertion())
    }
}
