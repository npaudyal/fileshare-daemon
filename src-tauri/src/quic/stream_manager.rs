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
                // BLAZING FAST: Handle direct file stream and write to disk immediately
                if let Err(e) = self.handle_blazing_fast_stream(recv).await {
                    error!("Failed to handle blazing fast stream: {}", e);
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
        
        for _ in 0..count {
            let _permit = self.stream_semaphore.acquire().await.unwrap();
            
            let mut send = self.connection.open_uni_stream().await?;
            
            // Write stream header
            QuicProtocol::write_stream_header(&mut send, StreamType::FileTransfer).await?;
            
            streams.push(send);
        }
        
        info!("Opened {} file transfer streams", count);
        
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
    
    // BLAZING FAST: Handle incoming file stream and write directly to disk
    async fn handle_blazing_fast_stream(&self, mut recv: RecvStream) -> Result<()> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        
        // Read file info header first
        let mut len_bytes = [0u8; 4];
        recv.read_exact(&mut len_bytes).await
            .map_err(|e| crate::FileshareError::Transfer(format!("Failed to read header: {}", e)))?;
        
        let info_len = u32::from_be_bytes(len_bytes) as usize;
        if info_len == 0 || info_len > 1024 {
            return Err(crate::FileshareError::Transfer("Invalid file info length".to_string()));
        }
        
        let mut info_bytes = vec![0u8; info_len];
        recv.read_exact(&mut info_bytes).await
            .map_err(|e| crate::FileshareError::Transfer(format!("Failed to read info: {}", e)))?;
        
        let file_info = String::from_utf8(info_bytes)
            .map_err(|e| crate::FileshareError::Transfer(format!("Invalid UTF8: {}", e)))?;
        
        // Parse file info: "FILEINFO|filename|filesize|target_path"
        let parts: Vec<&str> = file_info.split('|').collect();
        if parts.len() != 4 || parts[0] != "FILEINFO" {
            return Err(crate::FileshareError::Transfer("Invalid file info format".to_string()));
        }
        
        let filename = parts[1];
        let file_size: u64 = parts[2].parse()
            .map_err(|_| crate::FileshareError::Transfer("Invalid file size".to_string()))?;
        let target_path = parts[3];
        
        info!("🚀 BLAZING RECEIVE: {} ({:.1} MB) -> {}", 
              filename, file_size as f64 / (1024.0 * 1024.0), target_path);
        
        // Create target file
        let target_file_path = std::path::PathBuf::from(target_path);
        if let Some(parent) = target_file_path.parent() {
            tokio::fs::create_dir_all(parent).await
                .map_err(|e| crate::FileshareError::FileOperation(format!("Failed to create dir: {}", e)))?;
        }
        
        let mut file = tokio::fs::File::create(&target_file_path).await
            .map_err(|e| crate::FileshareError::FileOperation(format!("Failed to create file: {}", e)))?;
        
        // Read and write file data
        let start_time = std::time::Instant::now();
        let mut bytes_received = 0u64;
        let mut buffer = vec![0u8; 2 * 1024 * 1024]; // 2MB buffer for maximum speed
        
        loop {
            match recv.read(&mut buffer).await {
                Ok(Some(0)) | Ok(None) => break, // Stream ended
                Ok(Some(n)) => {
                    file.write_all(&buffer[0..n]).await
                        .map_err(|e| crate::FileshareError::FileOperation(format!("Failed to write: {}", e)))?;
                    bytes_received += n as u64;
                    
                    if bytes_received % (20 * 1024 * 1024) == 0 { // Log every 20MB
                        info!("📥 Received {:.1} MB of {:.1} MB", 
                              bytes_received as f64 / (1024.0 * 1024.0), 
                              file_size as f64 / (1024.0 * 1024.0));
                    }
                }
                Err(e) => {
                    error!("Stream read error: {}", e);
                    break;
                }
            }
        }
        
        file.flush().await
            .map_err(|e| crate::FileshareError::FileOperation(format!("Failed to flush: {}", e)))?;
        
        let duration = start_time.elapsed();
        let speed_mbps = if duration.as_secs_f64() > 0.0 {
            (bytes_received as f64 * 8.0) / (duration.as_secs_f64() * 1_000_000.0)
        } else { 0.0 };
        
        info!("🎉 BLAZING RECEIVE COMPLETE: {} ({:.1} MB in {:.2}s, {:.1} Mbps)", 
              filename, bytes_received as f64 / (1024.0 * 1024.0), 
              duration.as_secs_f64(), speed_mbps);
        
        Ok(())
    }
    
    // Close all streams and the connection
    pub async fn close(&self, error_code: u32, reason: &[u8]) {
        self.connection.connection.close(error_code.into(), reason);
        
        // Clear active streams
        let mut streams = self.active_streams.write().await;
        streams.clear();
    }
}