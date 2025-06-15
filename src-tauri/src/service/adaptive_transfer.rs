// src/service/adaptive_transfer.rs - COMPLETE FIXED VERSION
use crate::{config::Settings, network::protocol::*, FileshareError, Result};
use memmap2::{Mmap, MmapOptions};
use std::collections::HashMap;
use std::fs::File;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

// Simplified transfer configuration
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

// Simple transfer state
#[derive(Debug, Clone)]
pub enum TransferStatus {
    Starting,
    Active,
    Completed,
    Failed(String),
    Cancelled,
}

// Simplified progress tracking
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

// Main adaptive transfer manager
pub struct AdaptiveTransferManager {
    settings: Arc<Settings>,
    config: TransferConfig,
    active_transfers: HashMap<Uuid, Arc<TransferState>>,
    message_sender: Option<mpsc::UnboundedSender<(Uuid, Message)>>,
    // Store pending file offers for lookup
    pending_offers: HashMap<Uuid, PendingOffer>,
    // Store active incoming transfers for receiving
    incoming_transfers: HashMap<Uuid, IncomingTransfer>,
}

#[derive(Debug, Clone)]
struct PendingOffer {
    file_path: PathBuf,
    peer_id: Uuid,
    metadata: SimpleFileMetadata,
    created_at: Instant,
    peer_addr: Option<std::net::SocketAddr>, // Store peer address
}

#[derive(Debug)]
struct IncomingTransfer {
    transfer_id: Uuid,
    peer_id: Uuid,
    metadata: SimpleFileMetadata,
    save_path: PathBuf,
    expected_connections: usize,
    active_connections: Vec<TcpStream>,
    created_at: Instant,
}

// Internal transfer state with proper connection handling
struct TransferState {
    transfer_id: Uuid,
    peer_id: Uuid,
    file_path: PathBuf,
    total_bytes: u64,
    bytes_transferred: Arc<AtomicU64>, // Wrap in Arc for sharing
    started_at: Instant,
    status: std::sync::RwLock<TransferStatus>,
    connection_count: usize, // Store count instead of actual connections
}

impl AdaptiveTransferManager {
    pub async fn new(settings: Arc<Settings>) -> Result<Self> {
        Ok(Self {
            settings,
            config: TransferConfig::default(),
            active_transfers: HashMap::new(),
            message_sender: None,
            pending_offers: HashMap::new(),
            incoming_transfers: HashMap::new(),
        })
    }

    pub fn set_message_sender(&mut self, sender: mpsc::UnboundedSender<(Uuid, Message)>) {
        self.message_sender = Some(sender);
    }

    // MAIN ENTRY POINT: Send file with adaptive strategy (legacy method)
    pub async fn send_file(&mut self, peer_id: Uuid, file_path: PathBuf) -> Result<Uuid> {
        // This method now needs a peer address - it should be called via send_file_with_address
        Err(FileshareError::Transfer(
            "Use send_file_with_address instead - peer address required".to_string(),
        ))
    }

    // NEW: Send file with explicit peer address
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

        // Get file info
        let file_size = std::fs::metadata(&file_path)?.len();
        if file_size == 0 {
            return Err(FileshareError::FileOperation(
                "Cannot transfer empty file".to_string(),
            ));
        }

        // Create metadata with all required fields
        let metadata = SimpleFileMetadata {
            name: file_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
            size: file_size,
            transfer_id,
            checksum: None,                               // Optional field
            mime_type: Self::guess_mime_type(&file_path), // Add MIME type detection
            target_dir: None,                             // Optional field
        };

        info!(
            "📁 File: {} ({:.1}MB) -> {}",
            metadata.name,
            file_size as f64 / (1024.0 * 1024.0),
            peer_addr
        );

        // Store pending offer with peer address
        self.pending_offers.insert(
            transfer_id,
            PendingOffer {
                file_path: file_path.clone(),
                peer_id,
                metadata: metadata.clone(),
                created_at: Instant::now(),
                peer_addr: Some(peer_addr), // Store peer address
            },
        );

        // Send offer to peer
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

    // Guess MIME type helper
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

    // Handle incoming file offer
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

        // Create save path
        let save_path = self.get_save_path(&metadata.name, metadata.target_dir.as_deref())?;

        // Store incoming transfer info
        let incoming_transfer = IncomingTransfer {
            transfer_id,
            peer_id,
            metadata: metadata.clone(),
            save_path: save_path.clone(),
            expected_connections: self.calculate_optimal_params(metadata.size).1,
            active_connections: Vec::new(),
            created_at: Instant::now(),
        };

        self.incoming_transfers
            .insert(transfer_id, incoming_transfer);

        // Auto-accept (in real app, you might want user confirmation)
        if let Some(ref sender) = self.message_sender {
            let response = Message::new(MessageType::SimpleFileOfferResponse {
                transfer_id,
                accepted: true,
                my_port: self.settings.network.port,
            });

            sender
                .send((peer_id, response))
                .map_err(|e| FileshareError::Transfer(format!("Failed to send response: {}", e)))?;

            info!("✅ File offer accepted - will save to {:?}", save_path);
        }

        Ok(())
    }

    // Handle offer response and start transfer
    pub async fn handle_offer_response(
        &mut self,
        peer_id: Uuid,
        transfer_id: Uuid,
        accepted: bool,
        peer_port: u16,
    ) -> Result<()> {
        if !accepted {
            info!("❌ File offer rejected by peer");
            self.pending_offers.remove(&transfer_id);
            return Ok(());
        }

        info!("✅ File offer accepted, starting transfer");

        // Get pending offer
        let pending_offer = self
            .pending_offers
            .remove(&transfer_id)
            .ok_or_else(|| FileshareError::Transfer("Pending offer not found".to_string()))?;

        // Use stored peer address instead of looking it up
        let target_addr = if let Some(peer_addr) = pending_offer.peer_addr {
            let mut addr = peer_addr;
            addr.set_port(peer_port);
            addr
        } else {
            return Err(FileshareError::Transfer(
                "Peer address not stored in pending offer".to_string(),
            ));
        };

        info!("🎯 Connecting to peer at {}", target_addr);

        // Start the actual transfer
        self.start_outgoing_transfer(
            transfer_id,
            peer_id,
            target_addr,
            pending_offer.file_path,
            pending_offer.metadata.size,
        )
        .await
    }

    // Start outgoing file transfer
    async fn start_outgoing_transfer(
        &mut self,
        transfer_id: Uuid,
        peer_id: Uuid,
        target_addr: std::net::SocketAddr,
        file_path: PathBuf,
        file_size: u64,
    ) -> Result<()> {
        info!("🚀 Starting outgoing transfer to {}", target_addr);

        // Calculate optimal parameters
        let (chunk_size, connection_count) = self.calculate_optimal_params(file_size);

        info!(
            "📊 Using {} connections with {}KB chunks",
            connection_count,
            chunk_size / 1024
        );

        // Create connections
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

        // Create transfer state without storing connections
        let transfer_state = Arc::new(TransferState {
            transfer_id,
            peer_id,
            file_path: file_path.clone(),
            total_bytes: file_size,
            bytes_transferred: Arc::new(AtomicU64::new(0)), // Wrap in Arc
            started_at: Instant::now(),
            status: std::sync::RwLock::new(TransferStatus::Active),
            connection_count: connections.len(), // Store count, not connections
        });

        self.active_transfers
            .insert(transfer_id, transfer_state.clone());

        // Start the transfer based on file size
        if file_size > self.config.mmap_threshold {
            self.start_mmap_transfer(transfer_state, chunk_size, connections)
                .await
        } else {
            self.start_buffered_transfer(transfer_state, chunk_size, connections)
                .await
        }
    }

    // HIGH PERFORMANCE: Memory-mapped file transfer with proper connection handling
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

        // Distribute work directly to connections without cloning
        for (conn_id, stream) in connections.into_iter().enumerate() {
            let mmap_ref = Arc::clone(&mmap); // Clone the Arc, not the Mmap
            let bytes_counter = Arc::clone(&transfer_state.bytes_transferred); // Clone the Arc
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

        // Wait for all connections to complete
        for (i, handle) in handles.into_iter().enumerate() {
            match handle.await {
                Ok(Ok(_)) => {
                    info!("✅ Connection {} completed successfully", i);
                }
                Ok(Err(e)) => {
                    error!("❌ Connection {} failed: {}", i, e);
                }
                Err(e) => {
                    error!("❌ Connection {} task panicked: {}", i, e);
                }
            }
        }

        // Mark transfer as completed
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

    // DIRECT chunk sending - no pipeline overhead!
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

        // Send transfer ID first so receiver knows which transfer this is
        stream.write_all(transfer_id.as_bytes()).await?;

        for chunk_idx in start_chunk..end_chunk {
            let offset = chunk_idx * chunk_size;
            let end = std::cmp::min(offset + chunk_size, file_size);
            let actual_chunk_size = end - offset;

            // ZERO-COPY: Direct slice from mmap
            let chunk_data = &mmap[offset..end];

            // Simple protocol: chunk_index (8 bytes) + size (4 bytes) + data
            stream.write_u64(chunk_idx as u64).await?;
            stream.write_u32(actual_chunk_size as u32).await?;
            stream.write_all(chunk_data).await?;

            // Update progress
            bytes_counter.fetch_add(actual_chunk_size as u64, Ordering::Relaxed);

            // Periodic flush for better network utilization
            if chunk_idx % 10 == 0 {
                stream.flush().await?;
            }

            debug!(
                "📤 Connection {} sent chunk {} ({} bytes)",
                connection_id, chunk_idx, actual_chunk_size
            );
        }

        // Final flush
        stream.flush().await?;

        info!(
            "✅ Connection {} completed {} chunks",
            connection_id,
            end_chunk - start_chunk
        );
        Ok(())
    }

    // Fallback: Buffered transfer for smaller files
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

        // For smaller files, use single connection (take ownership)
        if let Some(mut stream) = connections.pop() {
            // Send transfer ID first
            stream
                .write_all(transfer_state.transfer_id.as_bytes())
                .await?;

            loop {
                let bytes_read = file.read(&mut buffer).await?;
                if bytes_read == 0 {
                    break;
                }

                // Send chunk
                stream.write_u64(chunk_index).await?;
                stream.write_u32(bytes_read as u32).await?;
                stream.write_all(&buffer[..bytes_read]).await?;

                // Update progress
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

        // Mark as completed
        {
            let mut status = transfer_state.status.write().unwrap();
            *status = TransferStatus::Completed;
        }

        info!("✅ Buffered transfer completed");
        Ok(())
    }

    // Handle incoming transfer connections
    pub async fn handle_incoming_connection(
        &mut self,
        mut stream: TcpStream,
        _peer_addr: std::net::SocketAddr,
    ) -> Result<()> {
        info!("📥 Handling incoming transfer connection");

        // Read transfer ID first
        let mut transfer_id_bytes = [0u8; 16];
        stream.read_exact(&mut transfer_id_bytes).await?;
        let transfer_id = Uuid::from_bytes(transfer_id_bytes);

        info!("📥 Incoming connection for transfer {}", transfer_id);

        // Find the incoming transfer
        if let Some(incoming) = self.incoming_transfers.get_mut(&transfer_id) {
            // Start receiving data on this connection
            let save_path = incoming.save_path.clone();
            let metadata = incoming.metadata.clone();

            tokio::spawn(async move {
                if let Err(e) =
                    Self::receive_chunks_on_connection(stream, save_path, metadata).await
                {
                    error!("❌ Failed to receive chunks: {}", e);
                }
            });

            info!(
                "✅ Started receiving on connection for transfer {}",
                transfer_id
            );
        } else {
            warn!("⚠️ No incoming transfer found for ID {}", transfer_id);
        }

        Ok(())
    }

    // Receive chunks on a single connection
    async fn receive_chunks_on_connection(
        mut stream: TcpStream,
        save_path: PathBuf,
        metadata: SimpleFileMetadata,
    ) -> Result<()> {
        info!("📥 Starting to receive chunks, saving to {:?}", save_path);

        let mut received_chunks = HashMap::new();
        let mut total_received = 0u64;

        // Create the output file
        let mut output_file = tokio::fs::File::create(&save_path).await?;

        loop {
            // Read chunk header
            let chunk_index = match stream.read_u64().await {
                Ok(idx) => idx,
                Err(_) => break, // Connection closed
            };

            let chunk_size = stream.read_u32().await? as usize;

            // Read chunk data
            let mut chunk_data = vec![0u8; chunk_size];
            stream.read_exact(&mut chunk_data).await?;

            // Store chunk (in a real implementation, you'd write chunks in order)
            received_chunks.insert(chunk_index, chunk_data);
            total_received += chunk_size as u64;

            debug!("📥 Received chunk {} ({} bytes)", chunk_index, chunk_size);

            // Check if we have all chunks (simplified - assumes sequential chunks)
            if total_received >= metadata.size {
                break;
            }
        }

        // Write all chunks to file in order
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

    // Calculate optimal parameters based on file size
    fn calculate_optimal_params(&self, file_size: u64) -> (usize, usize) {
        let file_mb = file_size / (1024 * 1024);

        let (chunk_size, connections) = match file_mb {
            0..=10 => (512 * 1024, 1),          // 512KB chunks, 1 connection
            11..=100 => (1024 * 1024, 2),       // 1MB chunks, 2 connections
            101..=1000 => (2 * 1024 * 1024, 3), // 2MB chunks, 3 connections
            _ => (4 * 1024 * 1024, 4),          // 4MB chunks, 4 connections
        };

        (
            chunk_size,
            std::cmp::min(connections, self.config.max_connections),
        )
    }

    // Optimize TCP socket for bulk transfer
    async fn optimize_tcp_socket(&self, stream: &TcpStream) -> Result<()> {
        stream.set_nodelay(true)?;
        Ok(())
    }

    // Get save path for incoming files
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

        // Handle duplicate files
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

    // Get transfer progress
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
                connections_active: state.connection_count, // Use stored count
                started_at: state.started_at,
                status,
            })
        } else {
            None
        }
    }

    // Cleanup completed transfers
    pub fn cleanup_completed_transfers(&mut self) {
        let initial_count = self.active_transfers.len();

        self.active_transfers.retain(|_, state| {
            let status = state.status.read().unwrap();
            !matches!(
                *status,
                TransferStatus::Completed | TransferStatus::Failed(_)
            )
        });

        // Clean up old pending offers
        let cutoff = Instant::now() - Duration::from_secs(300); // 5 minutes
        self.pending_offers
            .retain(|_, offer| offer.created_at > cutoff);

        // Clean up old incoming transfers
        self.incoming_transfers
            .retain(|_, transfer| transfer.created_at > cutoff);

        let removed = initial_count - self.active_transfers.len();
        if removed > 0 {
            info!("🧹 Cleaned up {} completed transfers", removed);
        }
    }

    // Get all active transfer progress
    pub fn get_all_active_progress(&self) -> Vec<TransferProgress> {
        self.active_transfers
            .keys()
            .filter_map(|&transfer_id| self.get_progress(transfer_id))
            .collect()
    }

    // Cancel a transfer
    pub fn cancel_transfer(&mut self, transfer_id: Uuid) -> Result<()> {
        if let Some(state) = self.active_transfers.get(&transfer_id) {
            let mut status = state.status.write().unwrap();
            *status = TransferStatus::Cancelled;
            info!("🛑 Transfer {} cancelled", transfer_id);
        }

        // Remove pending offer if exists
        self.pending_offers.remove(&transfer_id);

        // Remove incoming transfer if exists
        self.incoming_transfers.remove(&transfer_id);

        Ok(())
    }

    // Get transfer statistics
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

// Transfer statistics
#[derive(Debug, Clone)]
pub struct TransferStats {
    pub active_outgoing: usize,
    pub pending_offers: usize,
    pub incoming_transfers: usize,
    pub total_bytes_transferred: u64,
}
