use crate::network::{streaming_protocol::*, streaming_reader::*, streaming_writer::*};
use crate::{FileshareError, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, RwLock};
use tracing::{error, info, warn};
use uuid::Uuid;

pub struct StreamingTransferManager {
    config: StreamingConfig,
    active_outgoing: Arc<RwLock<HashMap<Uuid, StreamingChunkReader>>>,
    active_incoming: Arc<RwLock<HashMap<Uuid, StreamingFileWriter>>>,
    connections: Arc<RwLock<HashMap<Uuid, TcpStream>>>,
}

impl StreamingTransferManager {
    pub fn new(config: StreamingConfig) -> Self {
        Self {
            config,
            active_outgoing: Arc::new(RwLock::new(HashMap::new())),
            active_incoming: Arc::new(RwLock::new(HashMap::new())),
            connections: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    // Start streaming file to peer
    pub async fn start_streaming_transfer(
        &self,
        peer_id: Uuid,
        file_path: PathBuf,
        mut stream: TcpStream,
    ) -> Result<Uuid> {
        let transfer_id = Uuid::new_v4();

        // Create metadata
        let metadata = self.create_metadata(&file_path).await?;

        info!(
            "🚀 Starting streaming transfer {} to peer {}: {} ({} bytes)",
            transfer_id, peer_id, metadata.name, metadata.size
        );

        // Create streaming reader
        let reader = StreamingFileReader::new(file_path, transfer_id, self.config.clone()).await?;
        let mut chunk_stream = reader.create_chunk_stream().await?;

        // Store the reader for progress tracking
        {
            let mut outgoing = self.active_outgoing.write().await;
            outgoing.insert(transfer_id, chunk_stream);
        }

        // Start streaming in background
        let chunk_stream = {
            let mut outgoing = self.active_outgoing.write().await;
            outgoing.remove(&transfer_id).unwrap()
        };

        let streaming_config = self.config.clone();
        tokio::spawn(async move {
            if let Err(e) = Self::stream_file_chunks(chunk_stream, stream, streaming_config).await {
                error!("❌ Streaming transfer {} failed: {}", transfer_id, e);
            }
        });

        Ok(transfer_id)
    }

    // Stream file chunks over TCP
    async fn stream_file_chunks(
        mut chunk_stream: StreamingChunkReader,
        mut stream: TcpStream,
        _config: StreamingConfig,
    ) -> Result<()> {
        let mut total_bytes = 0u64;
        let mut chunk_count = 0u64;

        while let Some((header, data)) = chunk_stream.next_chunk().await? {
            // Send header first
            let header_bytes = header.to_bytes();
            stream
                .write_all(&header_bytes)
                .await
                .map_err(|e| FileshareError::Network(e))?;

            // Send chunk data
            stream
                .write_all(&data)
                .await
                .map_err(|e| FileshareError::Network(e))?;

            total_bytes += data.len() as u64;
            chunk_count += 1;

            if chunk_count % 100 == 0 {
                info!("📤 Streamed {} chunks ({} bytes)", chunk_count, total_bytes);
            }

            if header.is_last_chunk() {
                break;
            }
        }

        // Flush the stream
        stream
            .flush()
            .await
            .map_err(|e| FileshareError::Network(e))?;

        info!(
            "✅ Streaming transfer completed: {} chunks, {} bytes",
            chunk_count, total_bytes
        );
        Ok(())
    }

    // Receive streaming file from peer
    pub async fn receive_streaming_transfer(
        &self,
        transfer_id: Uuid,
        metadata: StreamingFileMetadata,
        save_path: PathBuf,
        mut stream: TcpStream,
    ) -> Result<()> {
        info!(
            "📥 Receiving streaming transfer {}: {} ({} bytes)",
            transfer_id, metadata.name, metadata.size
        );

        // Create streaming writer
        let mut writer = StreamingFileWriter::new(
            save_path,
            transfer_id,
            self.config.clone(),
            metadata.size,
            metadata.estimated_chunks,
        )
        .await?;

        // Store writer for progress tracking
        {
            let mut incoming = self.active_incoming.write().await;
            incoming.insert(transfer_id, writer);
        }

        // Receive chunks
        let mut writer = {
            let mut incoming = self.active_incoming.write().await;
            incoming.remove(&transfer_id).unwrap()
        };

        Self::receive_file_chunks(&mut writer, stream).await?;

        info!("✅ Streaming receive completed: {}", transfer_id);
        Ok(())
    }

    async fn receive_file_chunks(
        writer: &mut StreamingFileWriter,
        mut stream: TcpStream,
    ) -> Result<()> {
        let mut header_buffer = [0u8; StreamChunkHeader::SIZE];

        loop {
            // Read header
            stream
                .read_exact(&mut header_buffer)
                .await
                .map_err(|e| FileshareError::Network(e))?;

            let header = StreamChunkHeader::from_bytes(&header_buffer);

            // Read chunk data
            let mut chunk_data = vec![0u8; header.chunk_size as usize];
            stream
                .read_exact(&mut chunk_data)
                .await
                .map_err(|e| FileshareError::Network(e))?;

            let data = bytes::Bytes::from(chunk_data);

            // Write chunk
            let is_complete = writer.write_chunk(header, data).await?;

            if is_complete || header.is_last_chunk() {
                break;
            }
        }

        Ok(())
    }

    async fn create_metadata(&self, file_path: &PathBuf) -> Result<StreamingFileMetadata> {
        use sha2::{Digest, Sha256};

        let file = std::fs::File::open(file_path)
            .map_err(|e| FileshareError::FileOperation(format!("Cannot open file: {}", e)))?;

        let metadata = file
            .metadata()
            .map_err(|e| FileshareError::FileOperation(format!("Cannot get metadata: {}", e)))?;

        let file_size = metadata.len();
        let estimated_chunks = (file_size + self.config.base_chunk_size as u64 - 1)
            / self.config.base_chunk_size as u64;

        // Calculate file hash (in background)
        let file_path_clone = file_path.clone();
        let file_hash = tokio::task::spawn_blocking(move || -> Result<String> {
            use std::io::Read;
            let mut file = std::fs::File::open(file_path_clone)?;
            let mut hasher = Sha256::new();
            let mut buffer = [0; 8192];

            loop {
                let bytes_read = file.read(&mut buffer)?;
                if bytes_read == 0 {
                    break;
                }
                hasher.update(&buffer[..bytes_read]);
            }

            Ok(format!("{:x}", hasher.finalize()))
        })
        .await
        .map_err(|e| FileshareError::Unknown(format!("Hash calculation failed: {}", e)))??;

        let name = file_path
            .file_name()
            .ok_or_else(|| FileshareError::FileOperation("Invalid filename".to_string()))?
            .to_string_lossy()
            .to_string();

        Ok(StreamingFileMetadata {
            name,
            size: file_size,
            modified: metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs()),
            mime_type: None, // Could be enhanced later
            target_dir: None,
            suggested_chunk_size: self.config.base_chunk_size,
            supports_compression: self.config.enable_compression,
            estimated_chunks,
            file_hash,
        })
    }

    pub async fn get_outgoing_progress(&self, transfer_id: Uuid) -> Option<f32> {
        let outgoing = self.active_outgoing.read().await;
        outgoing.get(&transfer_id).map(|reader| reader.progress())
    }

    pub async fn get_incoming_progress(&self, transfer_id: Uuid) -> Option<f32> {
        let incoming = self.active_incoming.read().await;
        incoming.get(&transfer_id).map(|writer| writer.progress())
    }
}
