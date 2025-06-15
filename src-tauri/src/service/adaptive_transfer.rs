// src/service/adaptive_transfer.rs - FIXED VERSION
use crate::{config::Settings, network::protocol::*, FileshareError, Result};
use memmap2::{Mmap, MmapOptions};
use std::collections::HashMap;
use std::fs::File;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

// Add transfer server port offset
const TRANSFER_PORT_OFFSET: u16 = 100; // Transfer port = main_port + 100

#[derive(Debug, Clone)]
pub struct TransferConfig {
    pub max_connections: usize,
    pub chunk_size: usize,
    pub buffer_size: usize,
    pub mmap_threshold: u64,
}

impl Default for TransferConfig {
    fn default() -> Self {
        Self {
            max_connections: 4,
            chunk_size: 1024 * 1024,
            buffer_size: 4 * 1024 * 1024,
            mmap_threshold: 10 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone)]
pub enum TransferStatus {
    Starting,
    Active,
    Completed,
    Failed(String),
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct TransferProgress {
    pub transfer_id: Uuid,
    pub bytes_transferred: u64,
    pub total_bytes: u64,
    pub speed_mbps: f64,
    pub connections_active: usize,
    pub started_at: Instant,
    pub status: TransferStatus,
}

impl TransferProgress {
    pub fn percentage(&self) -> f64 {
        if self.total_bytes == 0 {
            return 100.0;
        }
        (self.bytes_transferred as f64 / self.total_bytes as f64) * 100.0
    }

    pub fn eta_seconds(&self) -> f64 {
        if self.speed_mbps <= 0.0 {
            return f64::INFINITY;
        }
        let remaining_mb = (self.total_bytes - self.bytes_transferred) as f64 / (1024.0 * 1024.0);
        remaining_mb / self.speed_mbps
    }
}

pub struct AdaptiveTransferManager {
    settings: Arc<Settings>,
    config: TransferConfig,
    active_transfers: HashMap<Uuid, Arc<TransferState>>,
    message_sender: Option<mpsc::UnboundedSender<(Uuid, Message)>>,
    pending_offers: HashMap<Uuid, PendingOffer>,
    incoming_transfers: HashMap<Uuid, IncomingTransfer>,

    // NEW: Separate transfer server
    transfer_listener: Option<TcpListener>,
    transfer_port: u16,
}

#[derive(Debug, Clone)]
struct PendingOffer {
    file_path: PathBuf,
    peer_id: Uuid,
    metadata: SimpleFileMetadata,
    created_at: Instant,
    peer_addr: Option<std::net::SocketAddr>,
}

#[derive(Debug)]
struct IncomingTransfer {
    transfer_id: Uuid,
    peer_id: Uuid,
    metadata: SimpleFileMetadata,
    save_path: PathBuf,
    expected_connections: usize,
    received_chunks: HashMap<u64, Vec<u8>>,
    created_at: Instant,
}

struct TransferState {
    transfer_id: Uuid,
    peer_id: Uuid,
    file_path: PathBuf,
    total_bytes: u64,
    bytes_transferred: Arc<AtomicU64>,
    started_at: Instant,
    status: std::sync::RwLock<TransferStatus>,
    connection_count: usize,
}

impl AdaptiveTransferManager {
    pub async fn new(settings: Arc<Settings>) -> Result<Self> {
        let transfer_port = settings.network.port + TRANSFER_PORT_OFFSET;

        // Create dedicated transfer listener
        let transfer_listener = TcpListener::bind(format!("0.0.0.0:{}", transfer_port))
            .await
            .map_err(|e| FileshareError::Network(e))?;

        info!("🚀 Transfer server listening on port {}", transfer_port);

        Ok(Self {
            settings,
            config: TransferConfig::default(),
            active_transfers: HashMap::new(),
            message_sender: None,
            pending_offers: HashMap::new(),
            incoming_transfers: HashMap::new(),
            transfer_listener: Some(transfer_listener),
            transfer_port,
        })
    }

    // NEW: Start transfer server
    pub async fn start_transfer_server(&mut self) -> Result<()> {
        if let Some(listener) = self.transfer_listener.take() {
            let incoming_transfers = Arc::new(tokio::sync::RwLock::new(HashMap::<
                Uuid,
                IncomingTransfer,
            >::new()));

            // Copy data for the server task
            let server_incoming = incoming_transfers.clone();

            tokio::spawn(async move {
                info!("📡 Transfer server started, accepting connections...");

                loop {
                    match listener.accept().await {
                        Ok((stream, addr)) => {
                            info!("🔗 Transfer connection from {}", addr);

                            let transfers = server_incoming.clone();
                            tokio::spawn(async move {
                                if let Err(e) =
                                    Self::handle_transfer_connection(stream, transfers).await
                                {
                                    error!("❌ Transfer connection error: {}", e);
                                }
                            });
                        }
                        Err(e) => {
                            error!("❌ Transfer server accept error: {}", e);
                            tokio::time::sleep(Duration::from_secs(1)).await;
                        }
                    }
                }
            });

            // Update local reference
            self.incoming_transfers = HashMap::new(); // Will be managed by the server task
        }

        Ok(())
    }

    // NEW: Handle dedicated transfer connections
    async fn handle_transfer_connection(
        mut stream: TcpStream,
        incoming_transfers: Arc<tokio::sync::RwLock<HashMap<Uuid, IncomingTransfer>>>,
    ) -> Result<()> {
        // Read transfer ID first
        let mut transfer_id_bytes = [0u8; 16];
        stream.read_exact(&mut transfer_id_bytes).await?;
        let transfer_id = Uuid::from_bytes(transfer_id_bytes);

        info!("📥 Transfer connection for transfer {}", transfer_id);

        // Find the incoming transfer
        let (save_path, metadata) = {
            let transfers = incoming_transfers.read().await;
            if let Some(incoming) = transfers.get(&transfer_id) {
                (incoming.save_path.clone(), incoming.metadata.clone())
            } else {
                warn!("⚠️ No incoming transfer found for ID {}", transfer_id);
                return Ok(());
            }
        };

        // Start receiving chunks
        Self::receive_chunks_dedicated(stream, save_path, metadata).await
    }

    // FIXED: Dedicated chunk receiving
    async fn receive_chunks_dedicated(
        mut stream: TcpStream,
        save_path: PathBuf,
        metadata: SimpleFileMetadata,
    ) -> Result<()> {
        info!(
            "📥 Starting dedicated chunk receiver, saving to {:?}",
            save_path
        );

        let mut received_chunks = HashMap::new();
        let mut total_received = 0u64;

        loop {
            // Read chunk header
            let chunk_index = match stream.read_u64().await {
                Ok(idx) => idx,
                Err(_) => break, // Connection closed
            };

            let chunk_size = stream.read_u32().await? as usize;

            // Safety check
            if chunk_size > 10 * 1024 * 1024 {
                // 10MB max chunk
                error!("❌ Chunk too large: {} bytes", chunk_size);
                break;
            }

            // Read chunk data
            let mut chunk_data = vec![0u8; chunk_size];
            stream.read_exact(&mut chunk_data).await?;

            // Store chunk
            received_chunks.insert(chunk_index, chunk_data);
            total_received += chunk_size as u64;

            debug!("📥 Received chunk {} ({} bytes)", chunk_index, chunk_size);

            // Check if we have all chunks
            if total_received >= metadata.size {
                break;
            }
        }

        // Write all chunks to file in order
        let mut output_file = tokio::fs::File::create(&save_path).await?;
        let mut sorted_chunks: Vec<_> = received_chunks.into_iter().collect();
        sorted_chunks.sort_by_key(|(idx, _)| *idx);

        for (chunk_idx, chunk_data) in sorted_chunks {
            use tokio::io::AsyncWriteExt;
            output_file.write_all(&chunk_data).await?;
            debug!("📝 Wrote chunk {} to file", chunk_idx);
        }

        output_file.flush().await?;
        info!("✅ File received successfully: {:?}", save_path);

        Ok(())
    }

    pub fn set_message_sender(&mut self, sender: mpsc::UnboundedSender<(Uuid, Message)>) {
        self.message_sender = Some(sender);
    }

    // FIXED: Use transfer port in offer response
    pub async fn send_file_with_address(
        &mut self,
        peer_id: Uuid,
        file_path: PathBuf,
        peer_addr: std::net::SocketAddr,
    ) -> Result<Uuid> {
        let transfer_id = Uuid::new_v4();

        info!(
            "🚀 Starting adaptive file transfer to {}: {:?}",
            peer_addr, file_path
        );

        let file_size = std::fs::metadata(&file_path)?.len();
        if file_size == 0 {
            return Err(FileshareError::FileOperation(
                "Cannot transfer empty file".to_string(),
            ));
        }

        let metadata = SimpleFileMetadata {
            name: file_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
            size: file_size,
            transfer_id,
            checksum: None,
            mime_type: Self::guess_mime_type(&file_path),
            target_dir: None,
        };

        info!(
            "📁 File: {} ({:.1}MB) -> {}",
            metadata.name,
            file_size as f64 / (1024.0 * 1024.0),
            peer_addr
        );

        self.pending_offers.insert(
            transfer_id,
            PendingOffer {
                file_path: file_path.clone(),
                peer_id,
                metadata: metadata.clone(),
                created_at: Instant::now(),
                peer_addr: Some(peer_addr),
            },
        );

        if let Some(ref sender) = self.message_sender {
            let offer = Message::new(MessageType::SimpleFileOffer {
                transfer_id,
                metadata: metadata.clone(),
            });

            sender
                .send((peer_id, offer))
                .map_err(|e| FileshareError::Transfer(format!("Failed to send offer: {}", e)))?;

            info!("✅ File offer sent to peer {} at {}", peer_id, peer_addr);
        } else {
            return Err(FileshareError::Transfer(
                "Message sender not configured".to_string(),
            ));
        }

        Ok(transfer_id)
    }

    // FIXED: Send transfer port in response
    pub async fn handle_file_offer(
        &mut self,
        peer_id: Uuid,
        transfer_id: Uuid,
        metadata: SimpleFileMetadata,
    ) -> Result<()> {
        info!(
            "📥 Received file offer: {} ({:.1}MB)",
            metadata.name,
            metadata.size as f64 / (1024.0 * 1024.0)
        );

        let save_path = self.get_save_path(&metadata.name, metadata.target_dir.as_deref())?;

        let incoming_transfer = IncomingTransfer {
            transfer_id,
            peer_id,
            metadata: metadata.clone(),
            save_path: save_path.clone(),
            expected_connections: self.calculate_optimal_params(metadata.size).1,
            received_chunks: HashMap::new(),
            created_at: Instant::now(),
        };

        self.incoming_transfers
            .insert(transfer_id, incoming_transfer);

        if let Some(ref sender) = self.message_sender {
            let response = Message::new(MessageType::SimpleFileOfferResponse {
                transfer_id,
                accepted: true,
                my_port: self.transfer_port, // Send transfer port, not main port
            });

            sender
                .send((peer_id, response))
                .map_err(|e| FileshareError::Transfer(format!("Failed to send response: {}", e)))?;

            info!("✅ File offer accepted - will save to {:?}", save_path);
        }

        Ok(())
    }

    // FIXED: Connect to transfer port
    pub async fn handle_offer_response(
        &mut self,
        peer_id: Uuid,
        transfer_id: Uuid,
        accepted: bool,
        peer_transfer_port: u16,
    ) -> Result<()> {
        if !accepted {
            info!("❌ File offer rejected by peer");
            self.pending_offers.remove(&transfer_id);
            return Ok(());
        }

        info!("✅ File offer accepted, starting transfer");

        let pending_offer = self
            .pending_offers
            .remove(&transfer_id)
            .ok_or_else(|| FileshareError::Transfer("Pending offer not found".to_string()))?;

        // Use peer's transfer port, not main port
        let target_addr = if let Some(peer_addr) = pending_offer.peer_addr {
            let mut addr = peer_addr;
            addr.set_port(peer_transfer_port); // Use transfer port
            addr
        } else {
            return Err(FileshareError::Transfer(
                "Peer address not stored in pending offer".to_string(),
            ));
        };

        info!("🎯 Connecting to peer transfer port at {}", target_addr);

        self.start_outgoing_transfer(
            transfer_id,
            peer_id,
            target_addr,
            pending_offer.file_path,
            pending_offer.metadata.size,
        )
        .await
    }

    // Rest of the implementation remains the same...
    // (keep all the existing methods like start_outgoing_transfer, start_mmap_transfer, etc.)

    fn guess_mime_type(file_path: &PathBuf) -> Option<String> {
        let extension = file_path.extension()?.to_str()?.to_lowercase();
        match extension.as_str() {
            "txt" => Some("text/plain".to_string()),
            "pdf" => Some("application/pdf".to_string()),
            "jpg" | "jpeg" => Some("image/jpeg".to_string()),
            "png" => Some("image/png".to_string()),
            "gif" => Some("image/gif".to_string()),
            "mp4" | "mov" | "avi" | "mkv" => Some("video/mp4".to_string()),
            "mp3" | "wav" | "flac" => Some("audio/mpeg".to_string()),
            "zip" | "rar" | "7z" => Some("application/zip".to_string()),
            "json" => Some("application/json".to_string()),
            "xml" => Some("application/xml".to_string()),
            "doc" | "docx" => Some("application/msword".to_string()),
            "xls" | "xlsx" => Some("application/vnd.ms-excel".to_string()),
            "ppt" | "pptx" => Some("application/vnd.ms-powerpoint".to_string()),
            _ => None,
        }
    }

    // Keep all other existing methods unchanged...
    async fn start_outgoing_transfer(
        &mut self,
        transfer_id: Uuid,
        peer_id: Uuid,
        target_addr: std::net::SocketAddr,
        file_path: PathBuf,
        file_size: u64,
    ) -> Result<()> {
        info!("🚀 Starting outgoing transfer to {}", target_addr);

        let (chunk_size, connection_count) = self.calculate_optimal_params(file_size);
        info!(
            "📊 Using {} connections with {}KB chunks",
            connection_count,
            chunk_size / 1024
        );

        let mut connections = Vec::new();
        for i in 0..connection_count {
            match TcpStream::connect(target_addr).await {
                Ok(stream) => {
                    self.optimize_tcp_socket(&stream).await?;
                    connections.push(stream);
                    debug!("✅ Connection {} established", i);
                }
                Err(e) => {
                    error!("❌ Failed to create connection {}: {}", i, e);
                    break;
                }
            }
        }

        if connections.is_empty() {
            return Err(FileshareError::Network(std::io::Error::new(
                std::io::ErrorKind::ConnectionRefused,
                "No connections could be established",
            )));
        }

        info!("🔗 Created {} connections", connections.len());

        let transfer_state = Arc::new(TransferState {
            transfer_id,
            peer_id,
            file_path: file_path.clone(),
            total_bytes: file_size,
            bytes_transferred: Arc::new(AtomicU64::new(0)),
            started_at: Instant::now(),
            status: std::sync::RwLock::new(TransferStatus::Active),
            connection_count: connections.len(),
        });

        self.active_transfers
            .insert(transfer_id, transfer_state.clone());

        if file_size > self.config.mmap_threshold {
            self.start_mmap_transfer(transfer_state, chunk_size, connections)
                .await
        } else {
            self.start_buffered_transfer(transfer_state, chunk_size, connections)
                .await
        }
    }

    async fn start_mmap_transfer(
        &self,
        transfer_state: Arc<TransferState>,
        chunk_size: usize,
        connections: Vec<TcpStream>,
    ) -> Result<()> {
        info!("🚀 Starting HIGH-PERFORMANCE memory-mapped transfer");

        let file = File::open(&transfer_state.file_path)?;
        let mmap = unsafe { MmapOptions::new().populate().map(&file)? };

        let file_size = mmap.len();
        let connection_count = connections.len();

        let chunk_count = (file_size + chunk_size - 1) / chunk_size;
        let chunks_per_connection = (chunk_count + connection_count - 1) / connection_count;

        info!(
            "📊 File: {:.1}MB, Chunks: {}, Per connection: {}",
            file_size as f64 / (1024.0 * 1024.0),
            chunk_count,
            chunks_per_connection
        );

        let mmap = Arc::new(mmap);
        let mut handles = Vec::new();

        for (conn_id, stream) in connections.into_iter().enumerate() {
            let mmap_ref = Arc::clone(&mmap);
            let bytes_counter = Arc::clone(&transfer_state.bytes_transferred);
            let transfer_id = transfer_state.transfer_id;

            let start_chunk = conn_id * chunks_per_connection;
            let end_chunk = std::cmp::min((conn_id + 1) * chunks_per_connection, chunk_count);

            let handle = tokio::spawn(async move {
                Self::send_chunks_direct(
                    stream,
                    mmap_ref,
                    start_chunk,
                    end_chunk,
                    chunk_size,
                    file_size,
                    bytes_counter,
                    transfer_id,
                    conn_id,
                )
                .await
            });

            handles.push(handle);
        }

        for (i, handle) in handles.into_iter().enumerate() {
            match handle.await {
                Ok(Ok(_)) => info!("✅ Connection {} completed successfully", i),
                Ok(Err(e)) => error!("❌ Connection {} failed: {}", i, e),
                Err(e) => error!("❌ Connection {} task panicked: {}", i, e),
            }
        }

        {
            let mut status = transfer_state.status.write().unwrap();
            *status = TransferStatus::Completed;
        }

        let duration = transfer_state.started_at.elapsed();
        let speed = (file_size as f64 / (1024.0 * 1024.0)) / duration.as_secs_f64();

        info!(
            "🎉 HIGH-PERFORMANCE transfer completed in {:.1}s at {:.1}MB/s",
            duration.as_secs_f64(),
            speed
        );

        Ok(())
    }

    async fn send_chunks_direct(
        mut stream: TcpStream,
        mmap: Arc<Mmap>,
        start_chunk: usize,
        end_chunk: usize,
        chunk_size: usize,
        file_size: usize,
        bytes_counter: Arc<AtomicU64>,
        transfer_id: Uuid,
        connection_id: usize,
    ) -> Result<()> {
        info!(
            "🔗 Connection {} handling chunks {}-{}",
            connection_id, start_chunk, end_chunk
        );

        // Send transfer ID first
        stream.write_all(transfer_id.as_bytes()).await?;

        for chunk_idx in start_chunk..end_chunk {
            let offset = chunk_idx * chunk_size;
            let end = std::cmp::min(offset + chunk_size, file_size);
            let actual_chunk_size = end - offset;

            let chunk_data = &mmap[offset..end];

            stream.write_u64(chunk_idx as u64).await?;
            stream.write_u32(actual_chunk_size as u32).await?;
            stream.write_all(chunk_data).await?;

            bytes_counter.fetch_add(actual_chunk_size as u64, Ordering::Relaxed);

            if chunk_idx % 10 == 0 {
                stream.flush().await?;
            }

            debug!(
                "📤 Connection {} sent chunk {} ({} bytes)",
                connection_id, chunk_idx, actual_chunk_size
            );
        }

        stream.flush().await?;
        info!(
            "✅ Connection {} completed {} chunks",
            connection_id,
            end_chunk - start_chunk
        );
        Ok(())
    }

    async fn start_buffered_transfer(
        &self,
        transfer_state: Arc<TransferState>,
        chunk_size: usize,
        mut connections: Vec<TcpStream>,
    ) -> Result<()> {
        info!("📦 Starting buffered transfer for smaller file");

        let mut file = tokio::fs::File::open(&transfer_state.file_path).await?;
        let mut buffer = vec![0u8; chunk_size];
        let mut chunk_index = 0u64;

        if let Some(mut stream) = connections.pop() {
            stream
                .write_all(transfer_state.transfer_id.as_bytes())
                .await?;

            loop {
                let bytes_read = file.read(&mut buffer).await?;
                if bytes_read == 0 {
                    break;
                }

                stream.write_u64(chunk_index).await?;
                stream.write_u32(bytes_read as u32).await?;
                stream.write_all(&buffer[..bytes_read]).await?;

                transfer_state
                    .bytes_transferred
                    .fetch_add(bytes_read as u64, Ordering::Relaxed);
                chunk_index += 1;

                debug!(
                    "📤 Sent buffered chunk {} ({} bytes)",
                    chunk_index, bytes_read
                );
            }

            stream.flush().await?;
        }

        {
            let mut status = transfer_state.status.write().unwrap();
            *status = TransferStatus::Completed;
        }

        info!("✅ Buffered transfer completed");
        Ok(())
    }

    fn calculate_optimal_params(&self, file_size: u64) -> (usize, usize) {
        let file_mb = file_size / (1024 * 1024);

        let (chunk_size, connections) = match file_mb {
            0..=10 => (512 * 1024, 1),
            11..=100 => (1024 * 1024, 2),
            101..=1000 => (2 * 1024 * 1024, 3),
            _ => (4 * 1024 * 1024, 4),
        };

        (
            chunk_size,
            std::cmp::min(connections, self.config.max_connections),
        )
    }

    async fn optimize_tcp_socket(&self, stream: &TcpStream) -> Result<()> {
        stream.set_nodelay(true)?;
        Ok(())
    }

    fn get_save_path(&self, filename: &str, target_dir: Option<&str>) -> Result<PathBuf> {
        let save_dir = if let Some(target_dir_str) = target_dir {
            let target_path = PathBuf::from(target_dir_str);
            if target_path.exists() && target_path.is_dir() {
                target_path
            } else {
                self.get_default_save_dir()
            }
        } else {
            self.get_default_save_dir()
        };

        if !save_dir.exists() {
            std::fs::create_dir_all(&save_dir)?;
        }

        let mut save_path = save_dir.join(filename);

        let mut counter = 1;
        while save_path.exists() {
            let stem = std::path::Path::new(filename)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("file");

            let extension = std::path::Path::new(filename)
                .extension()
                .and_then(|s| s.to_str())
                .map(|s| format!(".{}", s))
                .unwrap_or_default();

            let new_filename = format!("{} ({}){}", stem, counter, extension);
            save_path = save_dir.join(new_filename);
            counter += 1;
        }

        Ok(save_path)
    }

    fn get_default_save_dir(&self) -> PathBuf {
        directories::UserDirs::new()
            .and_then(|dirs| dirs.download_dir().map(|d| d.to_path_buf()))
            .unwrap_or_else(|| {
                directories::UserDirs::new()
                    .and_then(|dirs| dirs.document_dir().map(|d| d.to_path_buf()))
                    .unwrap_or_else(|| PathBuf::from("."))
            })
    }

    pub fn get_progress(&self, transfer_id: Uuid) -> Option<TransferProgress> {
        if let Some(state) = self.active_transfers.get(&transfer_id) {
            let bytes_transferred = state.bytes_transferred.load(Ordering::Relaxed);
            let elapsed = state.started_at.elapsed().as_secs_f64();
            let speed = if elapsed > 0.0 {
                (bytes_transferred as f64 / (1024.0 * 1024.0)) / elapsed
            } else {
                0.0
            };

            let status = state.status.read().unwrap().clone();

            Some(TransferProgress {
                transfer_id: state.transfer_id,
                bytes_transferred,
                total_bytes: state.total_bytes,
                speed_mbps: speed,
                connections_active: state.connection_count,
                started_at: state.started_at,
                status,
            })
        } else {
            None
        }
    }

    pub fn cleanup_completed_transfers(&mut self) {
        let initial_count = self.active_transfers.len();

        self.active_transfers.retain(|_, state| {
            let status = state.status.read().unwrap();
            !matches!(
                *status,
                TransferStatus::Completed | TransferStatus::Failed(_)
            )
        });

        let cutoff = Instant::now() - Duration::from_secs(300);
        self.pending_offers
            .retain(|_, offer| offer.created_at > cutoff);
        self.incoming_transfers
            .retain(|_, transfer| transfer.created_at > cutoff);

        let removed = initial_count - self.active_transfers.len();
        if removed > 0 {
            info!("🧹 Cleaned up {} completed transfers", removed);
        }
    }

    pub fn get_all_active_progress(&self) -> Vec<TransferProgress> {
        self.active_transfers
            .keys()
            .filter_map(|&transfer_id| self.get_progress(transfer_id))
            .collect()
    }

    pub fn cancel_transfer(&mut self, transfer_id: Uuid) -> Result<()> {
        if let Some(state) = self.active_transfers.get(&transfer_id) {
            let mut status = state.status.write().unwrap();
            *status = TransferStatus::Cancelled;
            info!("🛑 Transfer {} cancelled", transfer_id);
        }

        self.pending_offers.remove(&transfer_id);
        self.incoming_transfers.remove(&transfer_id);

        Ok(())
    }

    pub fn get_stats(&self) -> TransferStats {
        TransferStats {
            active_outgoing: self.active_transfers.len(),
            pending_offers: self.pending_offers.len(),
            incoming_transfers: self.incoming_transfers.len(),
            total_bytes_transferred: self
                .active_transfers
                .values()
                .map(|t| t.bytes_transferred.load(Ordering::Relaxed))
                .sum(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TransferStats {
    pub active_outgoing: usize,
    pub pending_offers: usize,
    pub incoming_transfers: usize,
    pub total_bytes_transferred: u64,
}
