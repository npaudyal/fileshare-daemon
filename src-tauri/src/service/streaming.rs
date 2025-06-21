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
        
        info!("🔧 READER_INIT: Created StreamingFileReader with chunk_size={} for file size={} bytes", 
              chunk_size, total_size);
        
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
    
    pub async fn seek_to_chunk(&mut self, chunk_index: u64) -> Result<()> {
        use tokio::io::AsyncSeekExt;
        
        let seek_position = chunk_index * self.chunk_size as u64;
        if seek_position >= self.total_size {
            return Err(FileshareError::FileOperation(format!(
                "Seek position {} beyond file size {}", seek_position, self.total_size
            )));
        }
        
        // Get the inner file from the BufReader to seek
        let file = self.file.get_mut();
        file.seek(std::io::SeekFrom::Start(seek_position)).await?;
        
        // Update our tracking
        self.bytes_read = seek_position;
        
        Ok(())
    }
    
    pub async fn read_chunk_at_index(&mut self, chunk_index: u64) -> Result<Option<(Vec<u8>, bool)>> {
        // Seek to the correct position first
        self.seek_to_chunk(chunk_index).await?;
        
        // Calculate the size for this specific chunk
        let start_position = chunk_index * self.chunk_size as u64;
        let remaining_bytes = self.total_size - start_position;
        let chunk_size_to_read = std::cmp::min(self.chunk_size as u64, remaining_bytes) as usize;
        
        if chunk_size_to_read == 0 {
            return Ok(None);
        }
        
        debug!("🔧 READER_INDEX: Reading chunk {} with target size {} bytes (remaining: {} bytes)", 
               chunk_index, chunk_size_to_read, remaining_bytes);
        
        let mut buffer = vec![0u8; chunk_size_to_read];
        let mut total_bytes_read = 0;
        
        // Keep reading until we fill the entire chunk or reach EOF
        while total_bytes_read < chunk_size_to_read {
            let bytes_read = self.file.read(&mut buffer[total_bytes_read..]).await?;
            if bytes_read == 0 {
                // EOF reached
                break;
            }
            total_bytes_read += bytes_read;
        }
        
        debug!("🔧 READER_INDEX: Successfully read {} bytes for chunk {} (target: {})", 
               total_bytes_read, chunk_index, chunk_size_to_read);
        
        if total_bytes_read == 0 {
            return Ok(None);
        }
        
        buffer.truncate(total_bytes_read);
        self.hasher.update(&buffer);
        
        let is_last = start_position + total_bytes_read as u64 >= self.total_size;
        
        // Apply compression if needed
        let compressed_data = if let Some(compression) = self.compression {
            self.compress_chunk(&buffer, compression)?
        } else {
            buffer
        };
        
        Ok(Some((compressed_data, is_last)))
    }

    pub async fn read_next_chunk(&mut self) -> Result<Option<(Vec<u8>, bool)>> {
        if self.bytes_read >= self.total_size {
            return Ok(None);
        }
        
        // Calculate how many bytes we should read for this chunk
        let remaining_bytes = self.total_size - self.bytes_read;
        let chunk_size_to_read = std::cmp::min(self.chunk_size as u64, remaining_bytes) as usize;
        
        debug!("🔧 READER: Reading chunk with target size {} bytes (remaining: {} bytes)", 
               chunk_size_to_read, remaining_bytes);
        
        let mut buffer = vec![0u8; chunk_size_to_read];
        let mut total_bytes_read = 0;
        
        // Keep reading until we fill the entire chunk or reach EOF
        while total_bytes_read < chunk_size_to_read {
            let bytes_read = self.file.read(&mut buffer[total_bytes_read..]).await?;
            if bytes_read == 0 {
                // EOF reached
                break;
            }
            total_bytes_read += bytes_read;
        }
        
        debug!("🔧 READER: Successfully read {} bytes for chunk (target: {})", 
               total_bytes_read, chunk_size_to_read);
        
        if total_bytes_read == 0 {
            return Ok(None);
        }
        
        buffer.truncate(total_bytes_read);
        self.bytes_read += total_bytes_read as u64;
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
        // Ensure we can buffer all chunks needed for the transfer
        let memory_based_limit = MAX_MEMORY_PER_TRANSFER / chunk_size;
        let max_buffered_chunks = memory_based_limit.max(total_chunks as usize);
        
        info!("📦 BUFFER_INIT: total_chunks={}, memory_limit={}, buffer_size={}", 
              total_chunks, memory_based_limit, max_buffered_chunks);
        
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
        if index % 10 == 0 || index < 5 {  // Only log every 10th chunk or first 5 chunks
            debug!("🔧 WRITE_CHUNK: Attempting to write chunk {} (expecting {})", index, self.next_write_index);
        }
        
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
            if index % 10 == 0 || index < 5 {  // Only log every 10th chunk or first 5 chunks
                debug!("✅ DIRECT_WRITE: Writing chunk {} immediately ({} bytes)", index, decompressed_data.len());
            }
            self.write_data(&decompressed_data).await?;
            self.next_write_index += 1;
            
            if index < 5 {
                debug!("🔄 DIRECT_WRITE: After writing chunk {}, next_write_index is now {}", index, self.next_write_index);
            }
            
            // Check if we have buffered chunks that can now be written
            self.flush_buffered_chunks().await?;
        } else if index > self.next_write_index {
            // Store chunk in a map keyed by chunk index
            // This avoids issues with buffer slot calculation when chunks arrive very out of order
            
            // For now, use the existing buffer but with corrected logic
            // Calculate the position in buffer where this chunk should go
            let gap = (index - self.next_write_index) as usize;
            
            // Ensure buffer is large enough
            if gap > self.chunks_buffer.len() {
                let new_size = gap.min(1000); // Cap at 1000 to prevent memory issues
                if gap > 1000 {
                    return Err(FileshareError::Transfer(format!(
                        "Chunk {} is too far ahead (expected {}), gap of {} exceeds buffer limit",
                        index, self.next_write_index, gap
                    )));
                }
                self.chunks_buffer.resize(new_size, None);
                info!("📦 Expanded buffer to {} slots for chunk {} (gap: {})", new_size, index, gap);
            }
            
            // Place chunk at the correct position
            // chunks_buffer[0] should hold next_write_index + 1
            // chunks_buffer[1] should hold next_write_index + 2, etc.
            let buffer_position = gap - 1;
            
            if index % 10 == 0 || gap > 10 || index < 5 {  // Log significant gaps and first 5
                info!("📦 BUFFER: Storing chunk {} at buffer[{}] (expecting {}, gap: {})", 
                      index, buffer_position, self.next_write_index, gap);
            }
            
            // Check if slot is already occupied (shouldn't happen with correct logic)
            if buffer_position < self.chunks_buffer.len() && self.chunks_buffer[buffer_position].is_some() {
                warn!("⚠️ BUFFER: Position {} already occupied when storing chunk {} - this indicates a bug!", 
                      buffer_position, index);
            }
            
            if buffer_position < self.chunks_buffer.len() {
                self.chunks_buffer[buffer_position] = Some(decompressed_data);
                
                if index < 5 {
                    // Show buffer state for debugging
                    let occupied_slots: Vec<usize> = self.chunks_buffer.iter()
                        .enumerate()
                        .filter_map(|(i, slot)| if slot.is_some() { Some(i) } else { None })
                        .collect();
                    debug!("📦 BUFFER_STATE: After storing chunk {}, occupied slots: {:?}", index, occupied_slots);
                }
            } else {
                error!("❌ Buffer position {} out of bounds for chunk {} (buffer size: {})", 
                       buffer_position, index, self.chunks_buffer.len());
                return Err(FileshareError::Transfer(format!(
                    "Buffer position calculation error for chunk {}",
                    index
                )));
            }
        }
        
        Ok(())
    }
    
    async fn flush_buffered_chunks(&mut self) -> Result<()> {
        let mut consecutive_flushes = 0;
        
        // Keep flushing chunks that are now ready to be written sequentially
        while !self.chunks_buffer.is_empty() && self.chunks_buffer[0].is_some() {
            // Get the first chunk from buffer (which should be the next expected chunk)
            if let Some(data) = self.chunks_buffer[0].take() {
                let chunk_index = self.next_write_index;
                
                if chunk_index % 10 == 0 || chunk_index < 5 {
                    debug!("🔄 FLUSH: Writing buffered chunk {} from buffer position 0", chunk_index);
                }
                
                // Write the chunk
                self.write_data(&data).await?;
                self.next_write_index += 1;
                consecutive_flushes += 1;
                
                // Shift all chunks one position left
                self.chunks_buffer.remove(0);
                self.chunks_buffer.push(None); // Add empty slot at the end
            } else {
                // First slot is empty, no more consecutive chunks
                break;
            }
        }
        
        if consecutive_flushes > 0 {
            info!("🔄 FLUSH: Flushed {} consecutive chunks ({}..{}), next expected: {}", 
                  consecutive_flushes, 
                  self.next_write_index - consecutive_flushes as u64,
                  self.next_write_index - 1,
                  self.next_write_index);
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
        info!("📝 FINALIZE: Starting finalization with {} bytes written, next_write_index: {}", 
              self.bytes_written, self.next_write_index);
        
        // Calculate expected total chunks from file size
        let expected_chunks = (self.total_size + self.chunk_size as u64 - 1) / self.chunk_size as u64;
        
        // Check if we have all chunks
        if self.next_write_index < expected_chunks {
            warn!("⚠️ FINALIZE: Missing chunks! Written: {}, Expected: {}", 
                  self.next_write_index, expected_chunks);
            
            // Try to flush any remaining buffered chunks
            self.flush_buffered_chunks().await?;
            
            // Check again after flush
            if self.next_write_index < expected_chunks {
                // Log which chunks are missing
                let missing_chunks: Vec<u64> = (self.next_write_index..expected_chunks).collect();
                error!("❌ FINALIZE: Still missing chunks after flush: {:?}", missing_chunks);
                
                // Check if any missing chunks are in the buffer
                for missing_idx in &missing_chunks {
                    let buffer_pos = (*missing_idx - self.next_write_index) as usize;
                    if buffer_pos < self.chunks_buffer.len() {
                        if let Some(data) = self.chunks_buffer[buffer_pos].take() {
                            warn!("🔧 FINALIZE: Found missing chunk {} in buffer at position {}", 
                                  missing_idx, buffer_pos);
                            // Write it now
                            self.write_data(&data).await?;
                            self.next_write_index += 1;
                        }
                    }
                }
            }
        }
        
        // Check if there are any remaining chunks in buffer (this shouldn't happen)
        let remaining_chunks: usize = self.chunks_buffer.iter().filter(|chunk| chunk.is_some()).count();
        if remaining_chunks > 0 {
            warn!("⚠️ FINALIZE: {} chunks still in buffer after final flush!", remaining_chunks);
            
            // Log buffer state for debugging
            for (i, chunk_opt) in self.chunks_buffer.iter().enumerate() {
                if chunk_opt.is_some() {
                    let chunk_index = self.next_write_index + i as u64 + 1;
                    warn!("🔧 FINALIZE: Buffer[{}] contains chunk {} (size: {} bytes)", 
                          i, chunk_index, chunk_opt.as_ref().unwrap().len());
                }
            }
            
            // Try one more flush
            self.flush_buffered_chunks().await?;
        }
        
        // Final flush of file buffer
        self.file.flush().await?;
        
        // Final validation
        if self.bytes_written != self.total_size {
            error!("❌ FINALIZE: Size mismatch! Written: {} bytes, Expected: {} bytes (diff: {} bytes)", 
                   self.bytes_written, self.total_size, 
                   (self.total_size as i64 - self.bytes_written as i64).abs());
        }
        
        info!("📝 FINALIZE: Completed with {} bytes written, expected: {} (chunks: {}/{})", 
              self.bytes_written, self.total_size, self.next_write_index, expected_chunks);
        
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