use crate::network::streaming_protocol::*;
use crate::{FileshareError, Result};
use bytes::{Bytes, BytesMut};
use lz4_flex::compress_prepend_size;
use memmap2::{Mmap, MmapOptions};
use std::fs::File;
use std::io::SeekFrom;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncSeekExt}; // FIXED: Added AsyncSeekExt
use tracing::{debug, info, warn};
use uuid::Uuid;

pub struct StreamingFileReader {
    config: StreamingConfig,
    file_path: PathBuf,
    file_size: u64,
    transfer_id: Uuid,
    use_mmap: bool,
}

impl StreamingFileReader {
    pub async fn new(
        file_path: PathBuf,
        transfer_id: Uuid,
        config: StreamingConfig, // Now Copy, so no move issues
    ) -> Result<Self> {
        let file = File::open(&file_path)
            .map_err(|e| FileshareError::FileOperation(format!("Cannot open file: {}", e)))?;

        let file_size = file
            .metadata()
            .map_err(|e| FileshareError::FileOperation(format!("Cannot get file metadata: {}", e)))?
            .len();

        // Use memory mapping for files larger than 10MB
        let use_mmap = file_size > 10 * 1024 * 1024;

        info!(
            "🚀 StreamingFileReader: {} file {} ({} bytes) - using {}",
            if use_mmap {
                "Memory-mapped"
            } else {
                "Buffered"
            },
            file_path.display(),
            file_size,
            if use_mmap { "mmap" } else { "traditional I/O" }
        );

        Ok(Self {
            config,
            file_path,
            file_size,
            transfer_id,
            use_mmap,
        })
    }

    pub fn file_size(&self) -> u64 {
        self.file_size
    }

    pub fn estimated_chunks(&self) -> u64 {
        (self.file_size + self.config.base_chunk_size as u64 - 1)
            / self.config.base_chunk_size as u64
    }

    // Create an async stream of compressed chunks
    pub async fn create_chunk_stream(self) -> Result<StreamingChunkReader> {
        if self.use_mmap {
            self.create_mmap_stream().await
        } else {
            self.create_buffered_stream().await
        }
    }

    async fn create_mmap_stream(self) -> Result<StreamingChunkReader> {
        let file = File::open(&self.file_path)
            .map_err(|e| FileshareError::FileOperation(format!("Cannot open file: {}", e)))?;

        let mmap = unsafe {
            MmapOptions::new()
                .map(&file)
                .map_err(|e| FileshareError::FileOperation(format!("Cannot mmap file: {}", e)))?
        };

        info!("📁 Memory-mapped file: {} bytes", mmap.len());

        Ok(StreamingChunkReader::new_mmap(
            Arc::new(mmap),
            self.transfer_id,
            self.config, // FIXED: Now uses Copy instead of move
        ))
    }

    async fn create_buffered_stream(self) -> Result<StreamingChunkReader> {
        let file = tokio::fs::File::open(&self.file_path)
            .await
            .map_err(|e| FileshareError::FileOperation(format!("Cannot open file: {}", e)))?;

        Ok(StreamingChunkReader::new_buffered(
            file,
            self.transfer_id,
            self.config, // FIXED: Now uses Copy instead of move
        ))
    }
}

pub enum StreamingChunkReader {
    Mmap {
        mmap: Arc<Mmap>,
        transfer_id: Uuid,
        config: StreamingConfig,
        current_offset: usize,
        total_size: usize,
        current_chunk: u64,
    },
    Buffered {
        file: tokio::fs::File,
        transfer_id: Uuid,
        config: StreamingConfig,
        current_chunk: u64,
        buffer: BytesMut,
    },
}

impl StreamingChunkReader {
    fn new_mmap(mmap: Arc<Mmap>, transfer_id: Uuid, config: StreamingConfig) -> Self {
        let total_size = mmap.len();
        Self::Mmap {
            mmap,
            transfer_id,
            config,
            current_offset: 0,
            total_size,
            current_chunk: 0,
        }
    }

    fn new_buffered(file: tokio::fs::File, transfer_id: Uuid, config: StreamingConfig) -> Self {
        Self::Buffered {
            file,
            transfer_id,
            config,
            current_chunk: 0,
            buffer: BytesMut::with_capacity(config.base_chunk_size),
        }
    }

    // Read next chunk with optional compression
    pub async fn next_chunk(&mut self) -> Result<Option<(StreamChunkHeader, Bytes)>> {
        match self {
            Self::Mmap {
                mmap,
                transfer_id,
                config,
                current_offset,
                total_size,
                current_chunk,
            } => {
                if *current_offset >= *total_size {
                    return Ok(None);
                }

                let chunk_size =
                    std::cmp::min(config.base_chunk_size, *total_size - *current_offset);

                let chunk_data = &mmap[*current_offset..*current_offset + chunk_size];
                let is_last = *current_offset + chunk_size >= *total_size;

                // Create header
                let mut header =
                    StreamChunkHeader::new(*transfer_id, *current_chunk, chunk_data.len() as u32);
                if is_last {
                    header.set_last_chunk();
                }

                // Compress if beneficial
                let final_data = if config.enable_compression
                    && chunk_data.len() > config.compression_threshold
                {
                    let compressed = tokio::task::spawn_blocking({
                        let data = chunk_data.to_vec();
                        move || compress_prepend_size(&data)
                    })
                    .await
                    .map_err(|e| {
                        FileshareError::Unknown(format!("Compression task failed: {}", e))
                    })?;

                    if compressed.len() < chunk_data.len() {
                        header.set_compressed();
                        header.chunk_size = compressed.len() as u32;
                        Bytes::from(compressed)
                    } else {
                        Bytes::copy_from_slice(chunk_data)
                    }
                } else {
                    Bytes::copy_from_slice(chunk_data)
                };

                // Calculate checksum
                header.calculate_checksum(&final_data);

                *current_offset += chunk_size;
                *current_chunk += 1;

                debug!(
                    "📦 Mmap chunk {}: {} bytes {} (offset: {}/{})",
                    header.get_chunk_index(), // FIXED: Use safe accessor
                    final_data.len(),
                    if header.is_compressed() {
                        "compressed"
                    } else {
                        "uncompressed"
                    },
                    *current_offset,
                    *total_size
                );

                Ok(Some((header, final_data)))
            }

            Self::Buffered {
                file,
                transfer_id,
                config,
                current_chunk,
                buffer,
            } => {
                let mut buffer = buffer;
                buffer.clear();
                buffer.resize(config.base_chunk_size, 0);

                let bytes_read = file
                    .read(&mut buffer)
                    .await
                    .map_err(|e| FileshareError::FileOperation(format!("Read error: {}", e)))?;

                if bytes_read == 0 {
                    return Ok(None);
                }

                buffer.truncate(bytes_read);

                // FIXED: Simplified last chunk detection without seeking
                // For streaming, we'll detect last chunk when we get 0 bytes on next read
                let mut header =
                    StreamChunkHeader::new(*transfer_id, *current_chunk, bytes_read as u32);

                // Note: We'll mark as last chunk only when we definitively know it's the last
                // This approach trades perfect last-chunk detection for simpler code

                // Compress if beneficial
                let final_data =
                    if config.enable_compression && buffer.len() > config.compression_threshold {
                        let compressed = tokio::task::spawn_blocking({
                            let data = buffer.to_vec();
                            move || compress_prepend_size(&data)
                        })
                        .await
                        .map_err(|e| {
                            FileshareError::Unknown(format!("Compression task failed: {}", e))
                        })?;

                        if compressed.len() < buffer.len() {
                            header.set_compressed();
                            header.chunk_size = compressed.len() as u32;
                            Bytes::from(compressed)
                        } else {
                            buffer.clone().freeze()
                        }
                    } else {
                        buffer.clone().freeze()
                    };

                header.calculate_checksum(&final_data);
                *current_chunk += 1;

                // Check if this might be the last chunk by reading ahead
                let mut peek_buffer = [0u8; 1];
                let is_last = match file.read(&mut peek_buffer).await {
                    Ok(0) => true, // EOF reached
                    Ok(_) => {
                        // We read a byte, seek back
                        if let Err(e) = file.seek(SeekFrom::Current(-1)).await {
                            warn!("Failed to seek back after peek: {}", e);
                        }
                        false
                    }
                    Err(_) => false, // Assume not last on error
                };

                if is_last {
                    header.set_last_chunk();
                }

                debug!(
                    "📦 Buffered chunk {}: {} bytes {} (last: {})",
                    header.get_chunk_index(), // FIXED: Use safe accessor
                    final_data.len(),
                    if header.is_compressed() {
                        "compressed"
                    } else {
                        "uncompressed"
                    },
                    is_last
                );

                Ok(Some((header, final_data)))
            }
        }
    }

    pub fn progress(&self) -> f32 {
        match self {
            Self::Mmap {
                current_offset,
                total_size,
                ..
            } => *current_offset as f32 / *total_size as f32,
            Self::Buffered { current_chunk, .. } => {
                // Rough estimate for buffered reading
                *current_chunk as f32 / 100.0
            }
        }
    }
}
