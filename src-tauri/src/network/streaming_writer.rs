use crate::network::streaming_protocol::*;
use crate::{FileshareError, Result};
use bytes::Bytes;
use lz4_flex::decompress_size_prepended;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::fs::File;
use tokio::io::{AsyncSeekExt, AsyncWriteExt, SeekFrom};
use tracing::{debug, info, warn};
use uuid::Uuid;

pub struct StreamingFileWriter {
    file: File,
    file_path: PathBuf,
    transfer_id: Uuid,
    config: StreamingConfig,
    expected_size: u64,
    bytes_written: u64,
    chunks_received: HashMap<u64, bool>,
    expected_chunks: u64,
    buffer: Vec<u8>,
    completed: bool,
}

impl StreamingFileWriter {
    pub async fn new(
        file_path: PathBuf,
        transfer_id: Uuid,
        config: StreamingConfig,
        expected_size: u64,
        expected_chunks: u64,
    ) -> Result<Self> {
        // Create parent directory if it doesn't exist
        if let Some(parent) = file_path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                FileshareError::FileOperation(format!("Cannot create directory: {}", e))
            })?;
        }

        let file = File::create(&file_path)
            .await
            .map_err(|e| FileshareError::FileOperation(format!("Cannot create file: {}", e)))?;

        // Pre-allocate file space for better performance
        file.set_len(expected_size)
            .await
            .map_err(|e| FileshareError::FileOperation(format!("Cannot set file length: {}", e)))?;

        info!(
            "📝 StreamingFileWriter: Created {} (pre-allocated {} bytes, expecting {} chunks)",
            file_path.display(),
            expected_size,
            expected_chunks
        );

        Ok(Self {
            file,
            file_path,
            transfer_id,
            config,
            expected_size,
            bytes_written: 0,
            chunks_received: HashMap::new(),
            expected_chunks,
            buffer: Vec::with_capacity(config.max_chunk_size),
            completed: false,
        })
    }

    pub async fn write_chunk(&mut self, header: StreamChunkHeader, data: Bytes) -> Result<bool> {
        if self.completed {
            warn!(
                "⚠️ Received chunk for already completed transfer {}",
                self.transfer_id
            );
            return Ok(true);
        }

        // FIXED: Use safe field access for packed struct
        let header_transfer_id = header.transfer_id();
        let chunk_index = header.get_chunk_index();
        let chunk_size = header.get_chunk_size();

        // Verify transfer ID
        if header_transfer_id != self.transfer_id {
            return Err(FileshareError::Transfer(
                "Chunk belongs to different transfer".to_string(),
            ));
        }

        // Verify checksum
        if !header.verify_checksum(&data) {
            return Err(FileshareError::Transfer(format!(
                "Checksum verification failed for chunk {}",
                chunk_index
            )));
        }

        // Check for duplicate chunks
        if self.chunks_received.contains_key(&chunk_index) {
            debug!("📦 Ignoring duplicate chunk {}", chunk_index);
            return Ok(self.is_transfer_complete());
        }

        // Decompress if needed
        let chunk_data = if header.is_compressed() {
            tokio::task::spawn_blocking({
                let data = data.to_vec();
                move || decompress_size_prepended(&data)
            })
            .await
            .map_err(|e| FileshareError::Unknown(format!("Decompression task failed: {}", e)))?
            .map_err(|e| FileshareError::Transfer(format!("Decompression failed: {}", e)))?
        } else {
            data.to_vec()
        };

        // Calculate file offset for this chunk
        let file_offset = chunk_index * self.config.base_chunk_size as u64;

        // Seek to correct position and write
        self.file
            .seek(SeekFrom::Start(file_offset))
            .await
            .map_err(|e| FileshareError::FileOperation(format!("Seek failed: {}", e)))?;

        self.file
            .write_all(&chunk_data)
            .await
            .map_err(|e| FileshareError::FileOperation(format!("Write failed: {}", e)))?;

        // Update tracking
        self.chunks_received.insert(chunk_index, true);
        self.bytes_written += chunk_data.len() as u64;

        debug!(
            "✅ Wrote chunk {} to offset {} ({} bytes) - {}/{} chunks received",
            chunk_index,
            file_offset,
            chunk_data.len(),
            self.chunks_received.len(),
            self.expected_chunks
        );

        // Check if transfer is complete
        let is_complete = header.is_last_chunk() || self.is_transfer_complete();

        if is_complete {
            self.finalize_transfer().await?;
        }

        Ok(is_complete)
    }

    fn is_transfer_complete(&self) -> bool {
        self.chunks_received.len() as u64 >= self.expected_chunks
    }

    async fn finalize_transfer(&mut self) -> Result<()> {
        if self.completed {
            return Ok(());
        }

        // Flush all writes
        self.file
            .flush()
            .await
            .map_err(|e| FileshareError::FileOperation(format!("Flush failed: {}", e)))?;

        // Sync to disk
        self.file
            .sync_all()
            .await
            .map_err(|e| FileshareError::FileOperation(format!("Sync failed: {}", e)))?;

        // Truncate file to actual size if needed
        if self.bytes_written < self.expected_size {
            self.file
                .set_len(self.bytes_written)
                .await
                .map_err(|e| FileshareError::FileOperation(format!("Truncate failed: {}", e)))?;
        }

        self.completed = true;

        info!(
            "🎉 Transfer {} completed: {} bytes written to {}",
            self.transfer_id,
            self.bytes_written,
            self.file_path.display()
        );

        Ok(())
    }

    pub fn progress(&self) -> f32 {
        if self.expected_chunks == 0 {
            return 0.0;
        }
        self.chunks_received.len() as f32 / self.expected_chunks as f32
    }

    pub fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    pub fn is_completed(&self) -> bool {
        self.completed
    }

    pub fn file_path(&self) -> &PathBuf {
        &self.file_path
    }
}

impl Drop for StreamingFileWriter {
    fn drop(&mut self) {
        if !self.completed {
            warn!(
                "🗑️ StreamingFileWriter dropped before completion: {}",
                self.file_path.display()
            );
        }
    }
}
