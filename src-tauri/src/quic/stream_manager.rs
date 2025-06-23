use crate::quic::protocol::*;
use crate::Result;
use quinn::{Connection, RecvStream, SendStream};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{mpsc, Mutex, RwLock};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Manages multiple QUIC streams per connection for parallel file transfers
pub struct StreamManager {
    connections: Arc<RwLock<HashMap<Uuid, PeerStreams>>>,
    chunk_senders: Arc<RwLock<HashMap<Uuid, mpsc::UnboundedSender<QuicChunk>>>>,
    chunk_receivers: Arc<RwLock<HashMap<Uuid, mpsc::UnboundedReceiver<QuicChunk>>>>,
}

#[derive(Debug)]
pub struct PeerStreams {
    peer_id: Uuid,
    connection: Connection,
    send_streams: HashMap<u8, SendStream>,
    recv_streams: HashMap<u8, RecvStream>,
    last_activity: Instant,
}

impl StreamManager {
    pub fn new() -> Self {
        Self {
            connections: Arc::new(RwLock::new(HashMap::new())),
            chunk_senders: Arc::new(RwLock::new(HashMap::new())),
            chunk_receivers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register a new peer connection with streams
    pub async fn register_peer(&self, peer_id: Uuid, connection: Connection) -> Result<()> {
        let peer_streams = PeerStreams {
            peer_id,
            connection,
            send_streams: HashMap::new(),
            recv_streams: HashMap::new(),
            last_activity: Instant::now(),
        };

        let mut connections = self.connections.write().await;
        connections.insert(peer_id, peer_streams);

        info!("✅ Registered peer {} with stream manager", peer_id);
        Ok(())
    }

    /// Open data streams for a peer
    pub async fn open_data_streams(&self, peer_id: Uuid, stream_count: u8) -> Result<()> {
        let mut connections = self.connections.write().await;
        if let Some(peer_streams) = connections.get_mut(&peer_id) {
            // Open bidirectional streams for data transfer
            for stream_id in 1..=stream_count {
                let (send_stream, recv_stream) =
                    peer_streams.connection.open_bi().await.map_err(|e| {
                        crate::FileshareError::Transfer(format!("Open stream error: {}", e))
                    })?;
                peer_streams.send_streams.insert(stream_id, send_stream);
                peer_streams.recv_streams.insert(stream_id, recv_stream);

                debug!("📊 Opened data stream {} for peer {}", stream_id, peer_id);
            }

            peer_streams.last_activity = Instant::now();
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

    /// Send a chunk via a specific stream
    pub async fn send_chunk(&self, peer_id: Uuid, stream_id: u8, chunk: QuicChunk) -> Result<()> {
        let mut connections = self.connections.write().await;
        if let Some(peer_streams) = connections.get_mut(&peer_id) {
            if let Some(send_stream) = peer_streams.send_streams.get_mut(&stream_id) {
                // Serialize chunk
                let chunk_data = chunk.serialize();

                // Send chunk header (size)
                let size_bytes = (chunk_data.len() as u32).to_le_bytes();

                use tokio::io::AsyncWriteExt;
                send_stream.write_all(&size_bytes).await.map_err(|e| {
                    crate::FileshareError::Transfer(format!("Failed to send chunk size: {}", e))
                })?;

                // Send chunk data
                send_stream.write_all(&chunk_data).await.map_err(|e| {
                    crate::FileshareError::Transfer(format!("Failed to send chunk data: {}", e))
                })?;

                debug!(
                    "📤 Sent chunk {} via stream {} to peer {}",
                    chunk.chunk_id, stream_id, peer_id
                );

                Ok(())
            } else {
                Err(crate::FileshareError::Transfer(format!(
                    "Stream {} not found for peer {}",
                    stream_id, peer_id
                )))
            }
        } else {
            Err(crate::FileshareError::Transfer(format!(
                "Peer {} not found",
                peer_id
            )))
        }
    }

    /// Start chunk receiver for a transfer
    pub async fn get_chunk_receiver(
        &self,
        transfer_id: Uuid,
    ) -> Result<mpsc::UnboundedReceiver<QuicChunk>> {
        let (tx, rx) = mpsc::unbounded_channel();

        let mut senders = self.chunk_senders.write().await;
        senders.insert(transfer_id, tx);

        Ok(rx)
    }

    /// Start receiving chunks from all streams for a peer
    pub async fn start_chunk_receivers(&self, peer_id: Uuid, transfer_id: Uuid) -> Result<()> {
        let mut connections = self.connections.write().await;
        if let Some(peer_streams) = connections.get_mut(&peer_id) {
            // Move recv streams out of the connection to avoid ownership issues
            let recv_streams: Vec<(u8, RecvStream)> = peer_streams.recv_streams.drain().collect();

            for (stream_id, recv_stream) in recv_streams {
                let chunk_senders = self.chunk_senders.clone();
                let transfer_id = transfer_id;
                let stream_id = stream_id;
                let peer_id = peer_id;

                tokio::spawn(async move {
                    Self::receive_stream_chunks(recv_stream, stream_id, transfer_id, chunk_senders)
                        .await;
                });
            }

            Ok(())
        } else {
            Err(crate::FileshareError::Transfer(format!(
                "Peer {} not found",
                peer_id
            )))
        }
    }

    async fn receive_stream_chunks(
        mut recv_stream: RecvStream,
        stream_id: u8,
        transfer_id: Uuid,
        chunk_senders: Arc<RwLock<HashMap<Uuid, mpsc::UnboundedSender<QuicChunk>>>>,
    ) {
        loop {
            // Read chunk size
            let mut size_buffer = [0u8; 4];
            use tokio::io::AsyncReadExt;
            match recv_stream.read_exact(&mut size_buffer).await {
                Ok(()) => {}
                Err(e) => {
                    debug!("📥 Stream {} ended: {}", stream_id, e);
                    break;
                }
            }

            let chunk_size = u32::from_le_bytes(size_buffer) as usize;
            if chunk_size > 10 * 1024 * 1024 {
                // 10MB max chunk size
                error!("❌ Invalid chunk size: {} bytes", chunk_size);
                break;
            }

            // Read chunk data
            let mut chunk_buffer = vec![0u8; chunk_size];
            match recv_stream.read_exact(&mut chunk_buffer).await {
                Ok(()) => {}
                Err(e) => {
                    error!("❌ Failed to read chunk data: {}", e);
                    break;
                }
            }

            // Deserialize chunk
            match QuicChunk::deserialize(&chunk_buffer) {
                Ok(chunk) => {
                    debug!(
                        "📥 Received chunk {} from stream {}",
                        chunk.chunk_id, stream_id
                    );

                    // Send to appropriate transfer receiver
                    let senders = chunk_senders.read().await;
                    if let Some(sender) = senders.get(&transfer_id) {
                        if let Err(e) = sender.send(chunk) {
                            error!("❌ Failed to forward chunk: {}", e);
                            break;
                        }
                    } else {
                        warn!("⚠️ No receiver for transfer {}", transfer_id);
                    }
                }
                Err(e) => {
                    error!("❌ Failed to deserialize chunk: {}", e);
                    continue;
                }
            }
        }
    }

    /// Get connection statistics
    pub async fn get_stream_stats(&self, peer_id: Uuid) -> Option<StreamStats> {
        let connections = self.connections.read().await;
        if let Some(peer_streams) = connections.get(&peer_id) {
            let quinn_stats = peer_streams.connection.stats();
            Some(StreamStats {
                peer_id,
                active_send_streams: peer_streams.send_streams.len() as u8,
                active_recv_streams: peer_streams.recv_streams.len() as u8,
                bytes_sent: quinn_stats.udp_tx.bytes,
                bytes_received: quinn_stats.udp_rx.bytes,
                rtt: quinn_stats.path.rtt,
                congestion_window: quinn_stats.path.cwnd,
                last_activity: peer_streams.last_activity,
            })
        } else {
            None
        }
    }

    /// Close streams for a peer
    pub async fn close_peer_streams(&self, peer_id: Uuid) -> Result<()> {
        let mut connections = self.connections.write().await;
        if let Some(mut peer_streams) = connections.remove(&peer_id) {
            // Close all send streams
            for (stream_id, mut send_stream) in peer_streams.send_streams.drain() {
                use tokio::io::AsyncWriteExt;
                if let Err(e) = send_stream.shutdown().await {
                    warn!("⚠️ Failed to close send stream {}: {}", stream_id, e);
                }
            }

            // Recv streams will be closed when dropped
            peer_streams.recv_streams.clear();

            // Close connection
            peer_streams
                .connection
                .close(0u32.into(), b"Transfer complete");

            info!("🧹 Closed all streams for peer {}", peer_id);
            Ok(())
        } else {
            Err(crate::FileshareError::Transfer(format!(
                "Peer {} not found",
                peer_id
            )))
        }
    }

    /// Cleanup inactive connections
    pub async fn cleanup_inactive(&self, timeout: std::time::Duration) {
        let mut connections = self.connections.write().await;
        let now = Instant::now();

        connections.retain(|peer_id, peer_streams| {
            if now.duration_since(peer_streams.last_activity) > timeout {
                info!("🧹 Cleaning up inactive peer {}", peer_id);
                peer_streams
                    .connection
                    .close(0u32.into(), b"Inactive timeout");
                false
            } else {
                true
            }
        });
    }
}

#[derive(Debug, Clone)]
pub struct StreamStats {
    pub peer_id: Uuid,
    pub active_send_streams: u8,
    pub active_recv_streams: u8,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub rtt: std::time::Duration,
    pub congestion_window: u64,
    pub last_activity: Instant,
}

impl Default for StreamManager {
    fn default() -> Self {
        Self::new()
    }
}
