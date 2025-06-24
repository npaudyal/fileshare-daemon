use crate::{FileshareError, Result};
use quinn::{Connection, Endpoint, RecvStream, SendStream};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;
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
        info!("Connecting to peer {} at {} via QUIC", peer_id, addr);
        
        // Create client config that accepts any certificate (for development)
        let client_config = create_client_config()?;
        
        let connecting = self.endpoint
            .connect_with(client_config, addr, "fileshare-daemon")
            .map_err(|e| FileshareError::Network(std::io::Error::new(
                std::io::ErrorKind::ConnectionRefused,
                format!("Failed to initiate connection: {}", e)
            )))?;
        
        let connection = connecting.await.map_err(|e| {
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
    
    let server_config = quinn::ServerConfig::with_single_cert(cert_chain, key_der)
        .map_err(|e| FileshareError::Network(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Failed to create server config: {}", e)
        )))?;
    
    Ok(server_config)
}

fn create_client_config() -> Result<quinn::ClientConfig> {
    // For development - accept any certificate
    let client_config = quinn::ClientConfig::with_platform_verifier();
    Ok(client_config)
}