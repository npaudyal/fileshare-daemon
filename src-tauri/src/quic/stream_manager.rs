use crate::quic::connection::QuicConnection;
use crate::quic::protocol::{QuicProtocol, StreamType};
use crate::network::protocol::Message;
use crate::{FileshareError, Result};
use quinn::{RecvStream, SendStream};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock, Semaphore};
use tracing::{debug, error, info, warn};
use uuid::Uuid;
use futures;

const MAX_CONCURRENT_STREAMS: usize = 16; // Maximum concurrent streams per connection
const STREAM_POOL_SIZE: usize = 8; // Pre-allocated stream pool size

pub struct StreamManager {
    connection: QuicConnection,
    active_streams: Arc<RwLock<HashMap<Uuid, StreamHandle>>>,
    stream_semaphore: Arc<Semaphore>,
    control_tx: mpsc::UnboundedSender<(Uuid, Message)>,
}

#[derive(Clone)]
struct StreamHandle {
    stream_type: StreamType,
    send_stream: Option<Arc<RwLock<SendStream>>>,
    recv_stream: Option<Arc<RwLock<RecvStream>>>,
}

impl StreamManager {
    pub fn new(connection: QuicConnection, control_tx: mpsc::UnboundedSender<(Uuid, Message)>) -> Self {
        Self {
            connection,
            active_streams: Arc::new(RwLock::new(HashMap::new())),
            stream_semaphore: Arc::new(Semaphore::new(MAX_CONCURRENT_STREAMS)),
            control_tx,
        }
    }
    
    // Start listening for incoming streams
    pub async fn start_stream_listener(self: Arc<Self>) {
        let connection1 = self.connection.clone();
        let connection2 = self.connection.clone();
        
        // Handle incoming bidirectional streams
        let bi_self = self.clone();
        tokio::spawn(async move {
            loop {
                match connection1.accept_bi_stream().await {
                    Ok((send, recv)) => {
                        let stream_self = bi_self.clone();
                        tokio::spawn(async move {
                            if let Err(e) = stream_self.handle_incoming_bi_stream(send, recv).await {
                                error!("Error handling incoming bi stream: {}", e);
                            }
                        });
                    }
                    Err(e) => {
                        error!("Error accepting bi stream: {}", e);
                        break;
                    }
                }
            }
        });
        
        // Handle incoming unidirectional streams
        let uni_self = self.clone();
        tokio::spawn(async move {
            loop {
                match connection2.accept_uni_stream().await {
                    Ok(recv) => {
                        let stream_self = uni_self.clone();
                        tokio::spawn(async move {
                            if let Err(e) = stream_self.handle_incoming_uni_stream(recv).await {
                                error!("Error handling incoming uni stream: {}", e);
                            }
                        });
                    }
                    Err(e) => {
                        error!("Error accepting uni stream: {}", e);
                        break;
                    }
                }
            }
        });
    }
    
    async fn handle_incoming_bi_stream(&self, send: SendStream, mut recv: RecvStream) -> Result<()> {
        // Read stream type header
        let stream_type = QuicProtocol::read_stream_header(&mut recv).await?;
        
        debug!("Received incoming bi stream of type: {:?}", stream_type);
        
        match stream_type {
            StreamType::Control => {
                // Handle control messages
                loop {
                    match QuicProtocol::read_message(&mut recv).await {
                        Ok(message) => {
                            // Get the peer ID from the connection's remote address
                            let peer_id = self.connection.get_peer_id();
                            if let Err(e) = self.control_tx.send((peer_id, message)) {
                                error!("Failed to forward control message: {}", e);
                                break;
                            }
                        }
                        Err(e) => {
                            debug!("Control stream ended: {}", e);
                            break;
                        }
                    }
                }
            }
            _ => {
                warn!("Unexpected bidirectional stream type: {:?}", stream_type);
            }
        }
        
        Ok(())
    }
    
    async fn handle_incoming_uni_stream(&self, mut recv: RecvStream) -> Result<()> {
        // Read stream type header
        let stream_type = QuicProtocol::read_stream_header(&mut recv).await?;
        
        debug!("Received incoming uni stream of type: {:?}", stream_type);
        
        match stream_type {
            StreamType::FileTransfer => {
                // Create ultra-fast transfer instance and handle stream
                let transfer = crate::quic::ultra_fast_transfer::UltraFastTransfer::new();
                if let Err(e) = transfer.receive_stream(recv).await {
                    error!("Failed to handle ultra-fast transfer: {}", e);
                }
            }
            _ => {
                warn!("Unexpected unidirectional stream type: {:?}", stream_type);
            }
        }
        
        Ok(())
    }
    
    // Open a control stream for bidirectional communication
    pub async fn open_control_stream(&self) -> Result<()> {
        let _permit = self.stream_semaphore.acquire().await.unwrap();
        
        let (mut send, recv) = self.connection.open_bi_stream().await?;
        
        // Write stream header
        QuicProtocol::write_stream_header(&mut send, StreamType::Control).await?;
        
        // Store the stream handle
        let handle = StreamHandle {
            stream_type: StreamType::Control,
            send_stream: Some(Arc::new(RwLock::new(send))),
            recv_stream: Some(Arc::new(RwLock::new(recv))),
        };
        
        let stream_id = Uuid::new_v4();
        let mut streams = self.active_streams.write().await;
        streams.insert(stream_id, handle);
        
        info!("Opened control stream: {}", stream_id);
        
        Ok(())
    }
    
    // Send a control message
    pub async fn send_control_message(&self, message: Message) -> Result<()> {
        // Find an active control stream
        let streams = self.active_streams.read().await;
        let control_stream = streams.values()
            .find(|h| matches!(h.stream_type, StreamType::Control))
            .cloned();
        drop(streams);
        
        if let Some(handle) = control_stream {
            if let Some(send_stream) = &handle.send_stream {
                let mut stream = send_stream.write().await;
                QuicProtocol::write_message(&mut stream, &message).await?;
                return Ok(());
            }
        }
        
        // If no control stream exists, create one and send
        self.open_control_stream().await?;
        
        // Now find the newly created control stream and send
        let streams = self.active_streams.read().await;
        let control_stream = streams.values()
            .find(|h| matches!(h.stream_type, StreamType::Control))
            .cloned();
        drop(streams);
        
        if let Some(handle) = control_stream {
            if let Some(send_stream) = &handle.send_stream {
                let mut stream = send_stream.write().await;
                QuicProtocol::write_message(&mut stream, &message).await?;
                return Ok(());
            }
        }
        
        Err(crate::FileshareError::Transfer("Failed to create control stream".to_string()))
    }
    
    // Open multiple file transfer streams for parallel chunk sending
    pub async fn open_file_transfer_streams(&self, count: usize) -> Result<Vec<SendStream>> {
        let mut streams = Vec::with_capacity(count);
        
        // Open all streams in parallel for faster setup
        let mut handles = Vec::new();
        for _ in 0..count {
            let conn = self.connection.clone();
            let permit = self.stream_semaphore.clone().acquire_owned().await.unwrap();
            
            handles.push(tokio::spawn(async move {
                let mut send = conn.open_uni_stream().await?;
                // Write stream header
                QuicProtocol::write_stream_header(&mut send, StreamType::FileTransfer).await?;
                drop(permit); // Release permit after stream is ready
                Ok::<SendStream, FileshareError>(send)
            }));
        }
        
        // Collect all streams
        for handle in handles {
            match handle.await {
                Ok(Ok(stream)) => streams.push(stream),
                Ok(Err(e)) => return Err(e),
                Err(e) => return Err(FileshareError::Transfer(format!("Failed to spawn task: {}", e))),
            }
        }
        
        info!("Opened {} file transfer streams in parallel", count);
        
        Ok(streams)
    }
    
    // Send file chunks in parallel across multiple streams
    pub async fn send_chunks_parallel(
        &self,
        transfer_id: Uuid,
        chunks: Vec<(u64, Vec<u8>, bool)>,
        max_streams: usize,
    ) -> Result<()> {
        let stream_count = max_streams.min(chunks.len()).max(1);
        let mut streams = self.open_file_transfer_streams(stream_count).await?;
        
        // Distribute chunks across streams
        let chunks_per_stream = (chunks.len() + stream_count - 1) / stream_count;
        let mut futures = Vec::new();
        
        for (stream_idx, stream) in streams.drain(..).enumerate() {
            let start_idx = stream_idx * chunks_per_stream;
            let end_idx = ((stream_idx + 1) * chunks_per_stream).min(chunks.len());
            
            if start_idx >= chunks.len() {
                break;
            }
            
            let stream_chunks: Vec<_> = chunks[start_idx..end_idx].to_vec();
            let transfer_id = transfer_id;
            
            let future = tokio::spawn(async move {
                let mut stream = stream;
                for (chunk_index, data, is_last) in stream_chunks {
                    if let Err(e) = QuicProtocol::write_chunk_direct(
                        &mut stream,
                        transfer_id,
                        chunk_index,
                        &data,
                        is_last,
                    ).await {
                        error!("Failed to send chunk {}: {}", chunk_index, e);
                        return Err(e);
                    }
                    
                    if chunk_index % 10 == 0 {
                        debug!("Sent chunk {} on stream {}", chunk_index, stream_idx);
                    }
                }
                
                // Finish the stream
                if let Err(e) = stream.finish() {
                    warn!("Failed to finish stream: {}", e);
                }
                
                Ok(())
            });
            
            futures.push(future);
        }
        
        // Wait for all streams to complete
        let results = futures::future::join_all(futures).await;
        
        for (idx, result) in results.iter().enumerate() {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(e)) => error!("Stream {} failed: {}", idx, e),
                Err(e) => error!("Stream {} panicked: {}", idx, e),
            }
        }
        
        Ok(())
    }
    
    // Get connection statistics
    pub fn stats(&self) -> u64 {
        self.connection.connection.stats().frame_rx.stream
    }
    
    // Check if connection is still alive
    pub fn is_alive(&self) -> bool {
        !self.connection.is_closed()
    }
    
    // Close all streams and the connection
    pub async fn close(&self, error_code: u32, reason: &[u8]) {
        self.connection.connection.close(error_code.into(), reason);
        
        // Clear active streams
        let mut streams = self.active_streams.write().await;
        streams.clear();
    }
}