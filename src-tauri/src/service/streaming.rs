use crate::{network::protocol::*, FileshareError, Result};
use crate::network::protocol::CompressionType;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::Arc;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

const BUFFER_SIZE: usize = 8 * 1024 * 1024; // 8MB buffer for streaming
const MAX_MEMORY_PER_TRANSFER: usize = 100 * 1024 * 1024; // 100MB max memory per transfer

pub struct StreamingFileReader {
    file: BufReader<File>,
    chunk_size: usize,
    total_size: u64,
    bytes_read: u64,
    hasher: Sha256,
    compression: Option<CompressionType>,
}

impl StreamingFileReader {
    pub async fn new(
        path: &Path,
        chunk_size: usize,
        compression: Option<CompressionType>,
    ) -> Result<Self> {
        let file = File::open(path).await?;
        let metadata = file.metadata().await?;
        let total_size = metadata.len();
        
        let buffered = BufReader::with_capacity(BUFFER_SIZE, file);
        
        Ok(Self {
            file: buffered,
            chunk_size,
            total_size,
            bytes_read: 0,
            hasher: Sha256::new(),
            compression,
        })
    }
    
    pub async fn read_next_chunk(&mut self) -> Result<Option<(Vec<u8>, bool)>> {
        if self.bytes_read >= self.total_size {
            return Ok(None);
        }
        
        let mut buffer = vec![0u8; self.chunk_size];
        let bytes_read = self.file.read(&mut buffer).await?;
        
        if bytes_read == 0 {
            return Ok(None);
        }
        
        buffer.truncate(bytes_read);
        self.bytes_read += bytes_read as u64;
        self.hasher.update(&buffer);
        
        let is_last = self.bytes_read >= self.total_size;
        
        // Apply compression if needed
        let compressed_data = if let Some(compression) = self.compression {
            self.compress_chunk(&buffer, compression)?
        } else {
            buffer
        };
        
        Ok(Some((compressed_data, is_last)))
    }
    
    fn compress_chunk(&self, data: &[u8], compression: CompressionType) -> Result<Vec<u8>> {
        match compression {
            CompressionType::None => Ok(data.to_vec()),
            CompressionType::Zstd => {
                let level = 3; // Balance between speed and compression
                zstd::encode_all(data, level)
                    .map_err(|e| FileshareError::FileOperation(format!("Compression failed: {}", e)))
            }
            CompressionType::Lz4 => {
                lz4::block::compress(data, Some(lz4::block::CompressionMode::DEFAULT), true)
                    .map_err(|e| FileshareError::FileOperation(format!("Compression failed: {}", e)))
            }
        }
    }
    
    pub fn get_checksum(self) -> String {
        format!("{:x}", self.hasher.finalize())
    }
    
    pub fn progress(&self) -> (u64, u64) {
        (self.bytes_read, self.total_size)
    }
}

#[derive(Debug)]
pub struct StreamingFileWriter {
    file: BufWriter<File>,
    chunk_size: usize,
    total_size: u64,
    bytes_written: u64,
    hasher: Sha256,
    decompression: Option<CompressionType>,
    chunks_buffer: Vec<Option<Vec<u8>>>,
    next_write_index: u64,
}

impl StreamingFileWriter {
    pub async fn new(
        path: &Path,
        total_size: u64,
        chunk_size: usize,
        total_chunks: u64,
        decompression: Option<CompressionType>,
    ) -> Result<Self> {
        // Create parent directory if it doesn't exist
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        
        let file = File::create(path).await?;
        let buffered = BufWriter::with_capacity(BUFFER_SIZE, file);
        
        // Limit chunks buffer size to prevent memory exhaustion
        let max_buffered_chunks = (MAX_MEMORY_PER_TRANSFER / chunk_size).min(total_chunks as usize);
        
        Ok(Self {
            file: buffered,
            chunk_size,
            total_size,
            bytes_written: 0,
            hasher: Sha256::new(),
            decompression,
            chunks_buffer: vec![None; max_buffered_chunks],
            next_write_index: 0,
        })
    }
    
    pub async fn write_chunk(&mut self, index: u64, data: Vec<u8>, compressed: bool) -> Result<()> {
        // Skip chunks that are already written (duplicate handling)
        if index < self.next_write_index {
            info!("⚠️ Skipping duplicate chunk {} (already processed up to {})", index, self.next_write_index - 1);
            return Ok(());
        }
        
        // Decompress if needed
        let decompressed_data = if compressed && self.decompression.is_some() {
            self.decompress_chunk(&data, self.decompression.unwrap())?
        } else {
            data
        };
        
        // If this is the next expected chunk, write it immediately
        if index == self.next_write_index {
            self.write_data(&decompressed_data).await?;
            self.next_write_index += 1;
            
            // Check if we have buffered chunks that can now be written
            self.flush_buffered_chunks().await?;
        } else if index > self.next_write_index {
            // Calculate buffer index for out-of-order chunks
            let buffer_offset = index - self.next_write_index;
            
            // Check if chunk is within reasonable buffering distance
            if buffer_offset < self.chunks_buffer.len() as u64 {
                let buffer_index = buffer_offset as usize;
                if buffer_index < self.chunks_buffer.len() {
                    if index % 10 == 0 {  // Only log every 10th buffered chunk
                        info!("📦 Buffering out-of-order chunk {} at buffer index {} (expecting {})", 
                              index, buffer_index, self.next_write_index);
                    }
                    self.chunks_buffer[buffer_index] = Some(decompressed_data);
                } else {
                    warn!("⚠️ Buffer index {} out of bounds (buffer size: {})", buffer_index, self.chunks_buffer.len());
                }
            } else {
                // Chunk is too far ahead - expand buffer if reasonable
                warn!("⚠️ Chunk {} is far ahead (expected around {}), expanding buffer", 
                      index, self.next_write_index);
                
                // Calculate how many slots we need
                let required_buffer_size = (buffer_offset as usize) + 1;
                
                if required_buffer_size <= 1000 { // Reasonable limit
                    // Expand buffer to accommodate this chunk
                    self.chunks_buffer.resize(required_buffer_size, None);
                    let buffer_index = buffer_offset as usize;
                    
                    if buffer_index < self.chunks_buffer.len() {
                        self.chunks_buffer[buffer_index] = Some(decompressed_data);
                        info!("📦 Expanded buffer to {} slots for chunk {}", self.chunks_buffer.len(), index);
                    } else {
                        return Err(FileshareError::Transfer(format!(
                            "Buffer index calculation error: index {} >= buffer size {}",
                            buffer_index, self.chunks_buffer.len()
                        )));
                    }
                } else {
                    return Err(FileshareError::Transfer(format!(
                        "Chunk {} is too far ahead (expected around {}) - would require buffer size {}",
                        index, self.next_write_index, required_buffer_size
                    )));
                }
            }
        }
        
        Ok(())
    }
    
    async fn flush_buffered_chunks(&mut self) -> Result<()> {
        while !self.chunks_buffer.is_empty() && self.chunks_buffer[0].is_some() {
            // Get the first chunk from buffer (which should be the next expected chunk)
            if let Some(data) = self.chunks_buffer[0].take() {
                // Write the chunk
                self.write_data(&data).await?;
                self.next_write_index += 1;
                
                // Shift all chunks one position left
                self.chunks_buffer.remove(0);
                self.chunks_buffer.push(None); // Add empty slot at the end
                
                if (self.next_write_index - 1) % 10 == 0 {  // Only log every 10th flush
                    info!("✅ Flushed buffered chunk {} from buffer", self.next_write_index - 1);
                }
            } else {
                // First slot is empty, no more consecutive chunks
                break;
            }
        }
        Ok(())
    }
    
    async fn write_data(&mut self, data: &[u8]) -> Result<()> {
        self.file.write_all(data).await?;
        self.hasher.update(data);
        self.bytes_written += data.len() as u64;
        Ok(())
    }
    
    fn decompress_chunk(&self, data: &[u8], compression: CompressionType) -> Result<Vec<u8>> {
        match compression {
            CompressionType::None => Ok(data.to_vec()),
            CompressionType::Zstd => {
                zstd::decode_all(data)
                    .map_err(|e| FileshareError::FileOperation(format!("Decompression failed: {}", e)))
            }
            CompressionType::Lz4 => {
                lz4::block::decompress(data, None)
                    .map_err(|e| FileshareError::FileOperation(format!("Decompression failed: {}", e)))
            }
        }
    }
    
    pub async fn finalize(mut self) -> Result<String> {
        // Ensure all buffered chunks are written before finalizing
        self.flush_buffered_chunks().await?;
        self.file.flush().await?;
        Ok(format!("{:x}", self.hasher.finalize()))
    }
    
    pub fn progress(&self) -> (u64, u64) {
        (self.bytes_written, self.total_size)
    }
}

// Progress tracking for UI updates
#[derive(Debug, Clone)]
pub struct TransferProgress {
    pub transfer_id: Uuid,
    pub bytes_transferred: u64,
    pub total_bytes: u64,
    pub chunks_completed: u64,
    pub total_chunks: u64,
    pub speed_bps: u64,
    pub eta_seconds: Option<u64>,
}

pub fn calculate_adaptive_chunk_size(file_size: u64) -> usize {
    match file_size {
        0..=10_485_760 => 256 * 1024,           // <= 10MB: 256KB chunks
        10_485_761..=104_857_600 => 1024 * 1024, // 10MB-100MB: 1MB chunks  
        104_857_601..=1_073_741_824 => 4 * 1024 * 1024, // 100MB-1GB: 4MB chunks
        _ => 8 * 1024 * 1024,                   // > 1GB: 8MB chunks
    }
}