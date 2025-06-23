use crate::{FileshareError, Result};
use crate::network::protocol::CompressionType;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::collections::HashMap;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

const BUFFER_SIZE: usize = 64 * 1024 * 1024; // 64MB buffer for streaming (MAXIMUM PERFORMANCE)
const MAX_MEMORY_PER_TRANSFER: usize = 100 * 1024 * 1024; // 100MB max memory per transfer (INCREASED)

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
        let chunk_start = std::time::Instant::now();
        
        if self.bytes_read >= self.total_size {
            return Ok(None);
        }
        
        // Calculate how many bytes we should read for this chunk
        let remaining_bytes = self.total_size - self.bytes_read;
        let chunk_size_to_read = std::cmp::min(self.chunk_size as u64, remaining_bytes) as usize;
        
        // PERFORMANCE: Allocation timing
        let alloc_start = std::time::Instant::now();
        let mut buffer = vec![0u8; chunk_size_to_read];
        let alloc_time = alloc_start.elapsed();
        
        // PERFORMANCE: File I/O timing
        let io_start = std::time::Instant::now();
        let mut total_bytes_read = 0;
        let mut read_operations = 0;
        
        // Keep reading until we fill the entire chunk or reach EOF
        while total_bytes_read < chunk_size_to_read {
            let read_op_start = std::time::Instant::now();
            let bytes_read = self.file.read(&mut buffer[total_bytes_read..]).await?;
            let read_op_time = read_op_start.elapsed();
            read_operations += 1;
            
            if bytes_read == 0 {
                // EOF reached
                break;
            }
            total_bytes_read += bytes_read;
            
            if read_operations <= 3 || read_op_time.as_millis() > 10 { // Log slow reads
                debug!("🔧 PERF_IO: Read op {} took {:.2}ms, got {} bytes", 
                       read_operations, read_op_time.as_millis(), bytes_read);
            }
        }
        
        let total_io_time = io_start.elapsed();
        
        if total_bytes_read == 0 {
            return Ok(None);
        }
        
        // PERFORMANCE: Buffer processing timing
        let process_start = std::time::Instant::now();
        buffer.truncate(total_bytes_read);
        self.bytes_read += total_bytes_read as u64;
        self.hasher.update(&buffer);
        
        let is_last = self.bytes_read >= self.total_size;
        
        // Apply compression if needed
        let compress_start = std::time::Instant::now();
        let compressed_data = if let Some(compression) = self.compression {
            self.compress_chunk(&buffer, compression)?
        } else {
            buffer
        };
        let compress_time = compress_start.elapsed();
        let process_time = process_start.elapsed();
        
        let total_chunk_time = chunk_start.elapsed();
        
        // Log detailed timing for performance analysis
        let chunk_num = self.bytes_read / self.chunk_size as u64;
        if chunk_num < 5 || total_chunk_time.as_millis() > 50 { // Log first 5 chunks or slow chunks
            info!("🔧 PERF_CHUNK: Chunk {} - TOTAL: {:.2}ms | ALLOC: {:.2}ms | IO: {:.2}ms ({} ops) | PROCESS: {:.2}ms | COMPRESS: {:.2}ms | SIZE: {}->{}",
                  chunk_num,
                  total_chunk_time.as_millis(),
                  alloc_time.as_millis(),
                  total_io_time.as_millis(),
                  read_operations,
                  process_time.as_millis(),
                  compress_time.as_millis(),
                  total_bytes_read,
                  compressed_data.len()
            );
        }
        
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
    chunks_buffer: HashMap<u64, Vec<u8>>,
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
        
        // Use HashMap for better chunk management - no size limits needed
        info!("📦 BUFFER_INIT: total_chunks={}, using HashMap for chunk storage", total_chunks);
        
        Ok(Self {
            file: buffered,
            chunk_size,
            total_size,
            bytes_written: 0,
            hasher: Sha256::new(),
            decompression,
            chunks_buffer: HashMap::new(),
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
            // Store chunk directly in HashMap by chunk index - much simpler!
            if index % 10 == 0 || index < 5 {  // Log significant gaps and first 5
                info!("📦 BUFFER: Storing chunk {} in HashMap (expecting {})", 
                      index, self.next_write_index);
            }
            
            // Check if chunk is already stored (shouldn't happen)
            if self.chunks_buffer.contains_key(&index) {
                warn!("⚠️ BUFFER: Chunk {} already stored - this indicates a duplicate!", index);
            }
            
            self.chunks_buffer.insert(index, decompressed_data);
            
            if index < 5 {
                // Show buffer state for debugging
                let stored_chunks: Vec<u64> = self.chunks_buffer.keys().cloned().collect();
                debug!("📦 BUFFER_STATE: After storing chunk {}, stored chunks: {:?}", index, stored_chunks);
            }
        }
        
        Ok(())
    }
    
    async fn flush_buffered_chunks(&mut self) -> Result<()> {
        let mut consecutive_flushes = 0;
        let start_write_index = self.next_write_index;
        
        // Keep writing chunks sequentially if they're available in the HashMap
        while let Some(data) = self.chunks_buffer.remove(&self.next_write_index) {
            let chunk_index = self.next_write_index;
            
            if chunk_index % 10 == 0 || chunk_index < 5 || chunk_index >= 25 {
                debug!("🔄 FLUSH: Writing buffered chunk {} from HashMap ({} bytes)", 
                       chunk_index, data.len());
            }
            
            // Write the chunk
            self.write_data(&data).await?;
            self.next_write_index += 1;
            consecutive_flushes += 1;
            
            // Debug: Show remaining chunks in HashMap for critical chunks
            if chunk_index < 5 || chunk_index >= 25 {
                let remaining_chunks: Vec<u64> = self.chunks_buffer.keys().cloned().collect();
                debug!("🔄 FLUSH: After writing chunk {}, remaining chunks in HashMap: {:?}", 
                       chunk_index, remaining_chunks);
            }
        }
        
        if consecutive_flushes > 0 {
            info!("🔄 FLUSH: Flushed {} consecutive chunks ({}..{}), next expected: {}", 
                  consecutive_flushes, 
                  start_write_index,
                  self.next_write_index - 1,
                  self.next_write_index);
        }
        
        Ok(())
    }
    
    async fn write_data(&mut self, data: &[u8]) -> Result<()> {
        let write_start = std::time::Instant::now();
        
        // Write data without immediate flush for performance
        self.file.write_all(data).await?;
        let write_time = write_start.elapsed();
        
        // Update hash and tracking
        self.hasher.update(data);
        self.bytes_written += data.len() as u64;
        
        // Only flush every 5 chunks or on last chunk to improve performance
        let chunk_index = self.bytes_written / (16 * 1024 * 1024); // Approximate chunk number
        if chunk_index % 5 == 0 || self.bytes_written >= self.total_size {
            let flush_start = std::time::Instant::now();
            self.file.flush().await?;
            let flush_time = flush_start.elapsed();
            info!("💾 DISK_PERF: Chunk ~{} - WRITE: {:.2}ms | FLUSH: {:.2}ms | SIZE: {}MB", 
                  chunk_index, write_time.as_millis(), flush_time.as_millis(), data.len() / (1024*1024));
        } else {
            info!("💾 DISK_PERF: Chunk ~{} - WRITE: {:.2}ms | NO_FLUSH | SIZE: {}MB", 
                  chunk_index, write_time.as_millis(), data.len() / (1024*1024));
        }
        
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
                
                // Show current buffer state
                let stored_chunks: Vec<u64> = self.chunks_buffer.keys().cloned().collect();
                warn!("🔧 FINALIZE: Chunks still in HashMap: {:?}", stored_chunks);
                
                // Check if any missing chunks are in the HashMap and write them directly
                for missing_idx in &missing_chunks {
                    if let Some(data) = self.chunks_buffer.remove(missing_idx) {
                        warn!("🔧 FINALIZE: Found missing chunk {} in HashMap, writing directly", missing_idx);
                        // Write it directly at the correct file position
                        use tokio::io::AsyncSeekExt;
                        let file_position = *missing_idx * self.chunk_size as u64;
                        self.file.seek(std::io::SeekFrom::Start(file_position)).await?;
                        self.file.write_all(&data).await?;
                        self.hasher.update(&data);
                        self.bytes_written += data.len() as u64;
                        info!("✅ FINALIZE: Successfully wrote missing chunk {} at file position {}", 
                              missing_idx, file_position);
                    }
                }
                
                // Update next_write_index to expected_chunks since we've handled all missing chunks
                self.next_write_index = expected_chunks;
            }
        }
        
        // Check if there are any remaining chunks in HashMap (this shouldn't happen)
        if !self.chunks_buffer.is_empty() {
            let remaining_chunks: Vec<u64> = self.chunks_buffer.keys().cloned().collect();
            warn!("⚠️ FINALIZE: {} chunks still in HashMap after final processing: {:?}", 
                  remaining_chunks.len(), remaining_chunks);
            
            // Write any remaining chunks directly
            for (chunk_index, data) in self.chunks_buffer.drain() {
                warn!("🔧 FINALIZE: Writing orphaned chunk {} directly", chunk_index);
                use tokio::io::AsyncSeekExt;
                let file_position = chunk_index * self.chunk_size as u64;
                self.file.seek(std::io::SeekFrom::Start(file_position)).await?;
                self.file.write_all(&data).await?;
                self.hasher.update(&data);
                self.bytes_written += data.len() as u64;
            }
        }
        
        // Final flush of file buffer - CRITICAL for data integrity
        info!("💾 FINALIZE: Starting final flush...");
        let final_flush_start = std::time::Instant::now();
        self.file.flush().await?;
        let final_flush_time = final_flush_start.elapsed();
        info!("💾 FINALIZE: Final flush completed in {:.2}ms", final_flush_time.as_millis());
        
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
        0..=10_485_760 => 2 * 1024 * 1024,               // <= 10MB: 2MB chunks (larger for speed)
        10_485_761..=104_857_600 => 8 * 1024 * 1024,    // 10MB-100MB: 8MB chunks (much larger)
        104_857_601..=1_073_741_824 => 16 * 1024 * 1024, // 100MB-1GB: 16MB chunks (maximum for 585MB files)
        _ => 32 * 1024 * 1024,                           // > 1GB: 32MB chunks (maximum throughput)
    }
}