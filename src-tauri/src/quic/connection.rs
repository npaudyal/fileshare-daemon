use crate::{FileshareError, Result};
use quinn::{Connection, Endpoint, RecvStream, SendStream};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info};
use uuid::Uuid;
use std::collections::HashMap;

#[derive(Clone)]
pub struct QuicConnection {
    pub connection: Connection,
    pub peer_id: Uuid,
    pub is_authenticated: bool,
}

impl QuicConnection {
    pub fn new(connection: Connection, peer_id: Uuid) -> Self {
        Self {
            connection,
            peer_id,
            is_authenticated: false,
        }
    }
    
    pub async fn open_bi_stream(&self) -> Result<(SendStream, RecvStream)> {
        self.connection
            .open_bi()
            .await
            .map_err(|e| FileshareError::Network(std::io::Error::new(
                std::io::ErrorKind::ConnectionAborted,
                format!("Failed to open bidirectional stream: {}", e)
            )))
    }
    
    pub async fn open_uni_stream(&self) -> Result<SendStream> {
        self.connection
            .open_uni()
            .await
            .map_err(|e| FileshareError::Network(std::io::Error::new(
                std::io::ErrorKind::ConnectionAborted,
                format!("Failed to open unidirectional stream: {}", e)
            )))
    }
    
    pub async fn accept_bi_stream(&self) -> Result<(SendStream, RecvStream)> {
        self.connection
            .accept_bi()
            .await
            .map_err(|e| FileshareError::Network(std::io::Error::new(
                std::io::ErrorKind::ConnectionAborted,
                format!("Failed to accept bidirectional stream: {}", e)
            )))
    }
    
    pub async fn accept_uni_stream(&self) -> Result<RecvStream> {
        self.connection
            .accept_uni()
            .await
            .map_err(|e| FileshareError::Network(std::io::Error::new(
                std::io::ErrorKind::ConnectionAborted,
                format!("Failed to accept unidirectional stream: {}", e)
            )))
    }
    
    pub fn is_closed(&self) -> bool {
        self.connection.close_reason().is_some()
    }
    
    pub fn remote_address(&self) -> SocketAddr {
        self.connection.remote_address()
    }
    
    pub fn get_peer_id(&self) -> Uuid {
        self.peer_id
    }
}

pub struct QuicConnectionManager {
    endpoint: Endpoint,
    connections: Arc<RwLock<HashMap<Uuid, QuicConnection>>>,
}

impl QuicConnectionManager {
    pub async fn new(bind_addr: SocketAddr) -> Result<Self> {
        // For development, we'll use a simple insecure configuration
        // In production, you would use proper certificates
        let server_config = create_server_config()?;
        
        // Create the QUIC endpoint
        let endpoint = Endpoint::server(server_config, bind_addr)
            .map_err(|e| FileshareError::Network(std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                format!("Failed to bind QUIC endpoint: {}", e)
            )))?;
        
        info!("QUIC endpoint listening on {}", bind_addr);
        
        Ok(Self {
            endpoint,
            connections: Arc::new(RwLock::new(HashMap::new())),
        })
    }
    
    pub async fn connect_to_peer(&self, addr: SocketAddr, peer_id: Uuid) -> Result<QuicConnection> {
        info!("🔗 Initiating QUIC connection to peer {} at {}", peer_id, addr);
        
        // Create client config that accepts any certificate (for development)
        let client_config = create_client_config()?;
        info!("✅ Created QUIC client config");
        
        info!("🔄 Attempting QUIC endpoint connection...");
        let connecting = self.endpoint
            .connect_with(client_config, addr, "fileshare-daemon")
            .map_err(|e| {
                error!("❌ Failed to initiate QUIC connection: {}", e);
                FileshareError::Network(std::io::Error::new(
                    std::io::ErrorKind::ConnectionRefused,
                    format!("Failed to initiate connection: {}", e)
                ))
            })?;
        
        info!("⏳ Waiting for QUIC connection to establish...");
        let connection = connecting.await.map_err(|e| {
            error!("❌ QUIC connection failed: {}", e);
            FileshareError::Network(std::io::Error::new(
                std::io::ErrorKind::ConnectionRefused,
                format!("Failed to connect to peer: {}", e)
            ))
        })?;
        
        info!("Successfully connected to peer {} via QUIC", peer_id);
        
        let quic_conn = QuicConnection::new(connection, peer_id);
        
        // Store the connection
        let mut connections = self.connections.write().await;
        connections.insert(peer_id, quic_conn.clone());
        
        Ok(quic_conn)
    }
    
    pub async fn accept_connection(&self) -> Result<QuicConnection> {
        let connecting = self.endpoint.accept().await.ok_or_else(|| {
            FileshareError::Network(std::io::Error::new(
                std::io::ErrorKind::Other,
                "Endpoint closed"
            ))
        })?;
        
        let connection = connecting.await.map_err(|e| {
            FileshareError::Network(std::io::Error::new(
                std::io::ErrorKind::ConnectionAborted,
                format!("Failed to accept connection: {}", e)
            ))
        })?;
        
        let remote_addr = connection.remote_address();
        info!("Accepted QUIC connection from {}", remote_addr);
        
        // Create connection with temporary ID (will be updated after handshake)
        let temp_id = Uuid::new_v4();
        let quic_conn = QuicConnection::new(connection, temp_id);
        
        Ok(quic_conn)
    }
    
    pub async fn get_connection(&self, peer_id: Uuid) -> Option<QuicConnection> {
        let connections = self.connections.read().await;
        connections.get(&peer_id).cloned()
    }
    
    pub async fn store_connection(&self, peer_id: Uuid, connection: QuicConnection) {
        let mut connections = self.connections.write().await;
        connections.insert(peer_id, connection);
    }
    
    pub async fn remove_connection(&self, peer_id: Uuid) {
        let mut connections = self.connections.write().await;
        connections.remove(&peer_id);
    }
    
    pub async fn get_all_connections(&self) -> Vec<(Uuid, QuicConnection)> {
        let connections = self.connections.read().await;
        connections.iter().map(|(id, conn)| (*id, conn.clone())).collect()
    }
    
    pub fn local_addr(&self) -> Result<SocketAddr> {
        self.endpoint.local_addr().map_err(|e| {
            FileshareError::Network(std::io::Error::new(
                std::io::ErrorKind::AddrNotAvailable,
                format!("Failed to get local address: {}", e)
            ))
        })
    }
}

// Create optimized transport config for high-throughput LAN transfers
fn create_optimized_transport_config() -> quinn::TransportConfig {
    let mut config = quinn::TransportConfig::default();
    
    // Maximize flow control windows for LAN environments
    config.stream_receive_window(quinn::VarInt::from_u32(128 * 1024 * 1024)); // 128MB per stream
    config.receive_window(quinn::VarInt::from_u32(1024 * 1024 * 1024)); // 1GB connection window
    config.send_window(1024 * 1024 * 1024); // 1GB send window
    
    // Allow many concurrent streams
    config.max_concurrent_bidi_streams(quinn::VarInt::from_u32(1024));
    config.max_concurrent_uni_streams(quinn::VarInt::from_u32(1024));
    
    // Increase datagram buffer for better throughput
    config.datagram_receive_buffer_size(Some(64 * 1024 * 1024)); // 64MB
    
    // Set longer idle timeout for stable connections
    config.max_idle_timeout(Some(quinn::VarInt::from_u32(300_000).into())); // 5 minutes
    
    // Disable keep-alive for LAN (unnecessary overhead)
    config.keep_alive_interval(None);
    
    // Use BBR congestion control for better throughput
    config.congestion_controller_factory(Arc::new(quinn::congestion::BbrConfig::default()));
    
    config
}

// Helper functions for creating configs
fn create_server_config() -> Result<quinn::ServerConfig> {
    // Generate a self-signed certificate for development
    let cert = rcgen::generate_simple_self_signed(vec![
        "localhost".to_string(),
        "fileshare-daemon".to_string(),
    ])?;
    
    let cert_der = cert.cert.der().to_vec();
    let priv_key = cert.key_pair.serialize_der();
    
    let cert_chain = vec![rustls_pki_types::CertificateDer::from(cert_der)];
    let key_der = rustls_pki_types::PrivateKeyDer::try_from(priv_key)
        .map_err(|e| FileshareError::Config(format!("Invalid private key: {}", e)))?;
    
    let mut server_config = quinn::ServerConfig::with_single_cert(cert_chain, key_der)
        .map_err(|e| FileshareError::Network(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Failed to create server config: {}", e)
        )))?;
    
    // Apply optimized transport config
    let transport_config = Arc::new(create_optimized_transport_config());
    server_config.transport_config(transport_config);
    
    Ok(server_config)
}

fn create_client_config() -> Result<quinn::ClientConfig> {
    // For development - accept any certificate (including self-signed)
    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
    use rustls::{DigitallySignedStruct, SignatureScheme};
    
    #[derive(Debug)]
    struct SkipServerVerification;

    impl ServerCertVerifier for SkipServerVerification {
        fn verify_server_cert(
            &self,
            _end_entity: &CertificateDer<'_>,
            _intermediates: &[CertificateDer<'_>],
            _server_name: &ServerName<'_>,
            _ocsp_response: &[u8],
            _now: UnixTime,
        ) -> std::result::Result<ServerCertVerified, rustls::Error> {
            Ok(ServerCertVerified::assertion())
        }

        fn verify_tls12_signature(
            &self,
            _message: &[u8],
            _cert: &CertificateDer<'_>,
            _dss: &DigitallySignedStruct,
        ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }

        fn verify_tls13_signature(
            &self,
            _message: &[u8],
            _cert: &CertificateDer<'_>,
            _dss: &DigitallySignedStruct,
        ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }

        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            vec![
                SignatureScheme::RSA_PKCS1_SHA1,
                SignatureScheme::ECDSA_SHA1_Legacy,
                SignatureScheme::RSA_PKCS1_SHA256,
                SignatureScheme::ECDSA_NISTP256_SHA256,
                SignatureScheme::RSA_PKCS1_SHA384,
                SignatureScheme::ECDSA_NISTP384_SHA384,
                SignatureScheme::RSA_PKCS1_SHA512,
                SignatureScheme::ECDSA_NISTP521_SHA512,
                SignatureScheme::RSA_PSS_SHA256,
                SignatureScheme::RSA_PSS_SHA384,
                SignatureScheme::RSA_PSS_SHA512,
                SignatureScheme::ED25519,
                SignatureScheme::ED448,
            ]
        }
    }

    let crypto = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(std::sync::Arc::new(SkipServerVerification))
        .with_no_client_auth();

    let mut client_config = quinn::ClientConfig::new(std::sync::Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(crypto)
            .map_err(|e| FileshareError::Config(format!("Failed to create QUIC client config: {}", e)))?,
    ));
    
    // Apply optimized transport config
    let transport_config = Arc::new(create_optimized_transport_config());
    client_config.transport_config(transport_config);
    
    Ok(client_config)
}