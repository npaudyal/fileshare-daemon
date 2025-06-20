use crate::{error::StreamingError, network::protocol::TransferChunk, FileshareError, Result};
use crc32fast::Hasher;
use std::collections::BTreeMap;
use std::path::PathBuf;
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufReader};
use tracing::{debug, error, info, warn};

// Streaming File Reader
pub struct StreamingFileReader {
    file: BufReader<File>,
    file_size: u64,
    current_position: u64,
    chunk_size: usize,
    buffer: Vec<u8>,
    file_path: PathBuf,
}

impl StreamingFileReader {
    pub async fn new(file_path: PathBuf, chunk_size: usize) -> Result<Self> {
        info!("🔍 Opening file for streaming read: {:?}", file_path);

        let file = File::open(&file_path).await.map_err(|e| {
            error!("Failed to open file {:?}: {}", file_path, e);
            FileshareError::FileOperation(format!("Cannot open file: {}", e))
        })?;

        let metadata = file.metadata().await.map_err(|e| {
            error!("Failed to get file metadata {:?}: {}", file_path, e);
            FileshareError::FileOperation(format!("Cannot read file metadata: {}", e))
        })?;

        let file_size = metadata.len();
        info!(
            "📊 File size: {} bytes, chunk size: {} bytes",
            file_size, chunk_size
        );

        // Validate chunk size
        if chunk_size < 1024 || chunk_size > 64 * 1024 * 1024 {
            return Err(StreamingError::InvalidChunkSize { size: chunk_size }.into());
        }

        let buffered_file = BufReader::with_capacity(chunk_size * 2, file);
        let buffer = vec![0u8; chunk_size];

        Ok(Self {
            file: buffered_file,
            file_size,
            current_position: 0,
            chunk_size,
            buffer,
            file_path,
        })
    }

    pub async fn read_next_chunk(&mut self) -> Result<Option<TransferChunk>> {
        if self.current_position >= self.file_size {
            debug!(
                "📄 Reached end of file at position {}",
                self.current_position
            );
            return Ok(None);
        }

        let bytes_to_read = std::cmp::min(
            self.chunk_size,
            (self.file_size - self.current_position) as usize,
        );

        debug!(
            "📖 Reading chunk at position {}, size: {}",
            self.current_position, bytes_to_read
        );

        let bytes_read = self
            .file
            .read(&mut self.buffer[..bytes_to_read])
            .await
            .map_err(|e| {
                error!(
                    "Failed to read from file at position {}: {}",
                    self.current_position, e
                );
                StreamingError::StreamingInterrupted {
                    position: self.current_position,
                }
            })?;

        if bytes_read == 0 {
            debug!("📄 No more data to read");
            return Ok(None);
        }

        if bytes_read != bytes_to_read {
            warn!(
                "⚠️ Read {} bytes but expected {}",
                bytes_read, bytes_to_read
            );
        }

        let chunk_data = self.buffer[..bytes_read].to_vec();
        let chunk_index = self.current_position / self.chunk_size as u64;

        // Calculate CRC32 for chunk validation
        let mut hasher = Hasher::new();
        hasher.update(&chunk_data);
        let checksum = hasher.finalize();

        self.current_position += bytes_read as u64;

        debug!(
            "✅ Read chunk {}: {} bytes, checksum: {:08x}",
            chunk_index, bytes_read, checksum
        );

        let chunk = TransferChunk {
            index: chunk_index,
            data: chunk_data,
            is_last: self.current_position >= self.file_size,
            checksum,
            compressed_size: None,
        };

        Ok(Some(chunk))
    }

    pub fn get_progress(&self) -> (u64, u64, f64) {
        let percentage = if self.file_size == 0 {
            100.0
        } else {
            (self.current_position as f64 / self.file_size as f64) * 100.0
        };
        (self.current_position, self.file_size, percentage)
    }

    pub fn file_size(&self) -> u64 {
        self.file_size
    }

    pub fn current_position(&self) -> u64 {
        self.current_position
    }
}

// Streaming File Writer
#[derive(Debug)]
pub struct StreamingFileWriter {
    file: File,
    expected_size: u64,
    bytes_written: u64,
    chunks_received: BTreeMap<u64, bool>,
    pending_chunks: BTreeMap<u64, Vec<u8>>,
    next_expected_chunk: u64,
    chunk_size: usize,
    file_path: PathBuf,
}

impl StreamingFileWriter {
    pub async fn new(file_path: PathBuf, expected_size: u64, chunk_size: usize) -> Result<Self> {
        info!(
            "💾 Creating file for streaming write: {:?} (size: {} bytes)",
            file_path, expected_size
        );

        // Check available disk space
        let available_space = Self::get_available_disk_space(&file_path).await?;
        let required_space = expected_size + (100 * 1024 * 1024); // 100MB buffer

        if available_space < required_space {
            error!(
                "❌ Insufficient disk space: need {} bytes, available {} bytes",
                required_space, available_space
            );
            return Err(StreamingError::InsufficientDiskSpace {
                needed: required_space,
                available: available_space,
            }
            .into());
        }

        // Create parent directories if they don't exist
        if let Some(parent) = file_path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                error!(
                    "Failed to create parent directories for {:?}: {}",
                    file_path, e
                );
                FileshareError::FileOperation(format!("Cannot create directories: {}", e))
            })?;
        }

        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&file_path)
            .await
            .map_err(|e| {
                error!("Failed to create file {:?}: {}", file_path, e);
                FileshareError::FileOperation(format!("Cannot create file: {}", e))
            })?;

        // Pre-allocate file space for better performance (only for reasonably sized files)
        if expected_size > 0 && expected_size < 10 * 1024 * 1024 * 1024 {
            // < 10GB
            if let Err(e) = file.set_len(expected_size).await {
                warn!("⚠️ Could not pre-allocate file space: {}", e);
                // Continue anyway - not critical
            } else {
                debug!("📦 Pre-allocated {} bytes for file", expected_size);
            }
        }

        Ok(Self {
            file,
            expected_size,
            bytes_written: 0,
            chunks_received: BTreeMap::new(),
            pending_chunks: BTreeMap::new(),
            next_expected_chunk: 0,
            chunk_size,
            file_path,
        })
    }

    pub async fn write_chunk(&mut self, chunk: TransferChunk) -> Result<bool> {
        debug!(
            "📝 Processing chunk {}: {} bytes",
            chunk.index,
            chunk.data.len()
        );

        // Validate chunk checksum
        let mut hasher = Hasher::new();
        hasher.update(&chunk.data);
        let calculated_checksum = hasher.finalize();

        if calculated_checksum != chunk.checksum {
            error!(
                "❌ Chunk validation failed for chunk {}: expected {:08x}, got {:08x}",
                chunk.index, chunk.checksum, calculated_checksum
            );
            return Err(StreamingError::ChunkValidationFailed {
                index: chunk.index,
                expected: chunk.checksum,
                actual: calculated_checksum,
            }
            .into());
        }

        debug!("✅ Chunk {} validation passed", chunk.index);

        // Check if this is the next expected chunk
        if chunk.index == self.next_expected_chunk {
            // Write this chunk immediately
            self.write_chunk_to_file(&chunk).await?;
            self.next_expected_chunk += 1;

            debug!("📝 Wrote chunk {} in sequence", chunk.index);

            // Check if any pending chunks can now be written
            while let Some(pending_data) = self.pending_chunks.remove(&self.next_expected_chunk) {
                let pending_chunk = TransferChunk {
                    index: self.next_expected_chunk,
                    data: pending_data,
                    is_last: false, // Will be set correctly later
                    checksum: 0,    // Already validated
                    compressed_size: None,
                };
                self.write_chunk_to_file(&pending_chunk).await?;
                debug!(
                    "📝 Wrote pending chunk {} after gap",
                    self.next_expected_chunk
                );
                self.next_expected_chunk += 1;
            }
        } else if chunk.index > self.next_expected_chunk {
            // Store for later writing (out-of-order chunk)
            debug!(
                "📦 Storing out-of-order chunk {} (expected {})",
                chunk.index, self.next_expected_chunk
            );
            self.pending_chunks.insert(chunk.index, chunk.data);
        } else {
            // Duplicate chunk - ignore
            debug!("🔄 Ignoring duplicate chunk {}", chunk.index);
        }

        self.chunks_received.insert(chunk.index, true);

        // Check if transfer is complete
        let is_complete = chunk.is_last && self.pending_chunks.is_empty();

        if is_complete {
            info!("🎉 Transfer complete: {} bytes written", self.bytes_written);
        }

        Ok(is_complete)
    }

    async fn write_chunk_to_file(&mut self, chunk: &TransferChunk) -> Result<()> {
        let offset = chunk.index * self.chunk_size as u64;

        debug!("💾 Writing chunk {} at offset {}", chunk.index, offset);

        self.file
            .seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(|e| {
                error!(
                    "Failed to seek to offset {} for chunk {}: {}",
                    offset, chunk.index, e
                );
                FileshareError::FileOperation(format!("Cannot seek in file: {}", e))
            })?;

        self.file.write_all(&chunk.data).await.map_err(|e| {
            error!("Failed to write chunk {} data: {}", chunk.index, e);
            FileshareError::FileOperation(format!("Cannot write to file: {}", e))
        })?;

        self.file.flush().await.map_err(|e| {
            error!("Failed to flush chunk {} data: {}", chunk.index, e);
            FileshareError::FileOperation(format!("Cannot flush file: {}", e))
        })?;

        self.bytes_written += chunk.data.len() as u64;

        debug!(
            "✅ Chunk {} written successfully ({} total bytes)",
            chunk.index, self.bytes_written
        );

        Ok(())
    }

    async fn get_available_disk_space(path: &PathBuf) -> Result<u64> {
        // For now, return a large value - in production you'd implement platform-specific disk space checking
        // This is a simplified version to keep the implementation focused on streaming
        debug!("💽 Checking disk space for {:?}", path);

        #[cfg(target_os = "windows")]
        {
            // Windows implementation would use GetDiskFreeSpaceEx
            Ok(100 * 1024 * 1024 * 1024) // Assume 100GB available
        }
        #[cfg(not(target_os = "windows"))]
        {
            // Unix implementation would use statvfs
            Ok(100 * 1024 * 1024 * 1024) // Assume 100GB available
        }
    }

    pub fn get_progress(&self) -> (u64, u64, f64) {
        let percentage = if self.expected_size == 0 {
            100.0
        } else {
            (self.bytes_written as f64 / self.expected_size as f64) * 100.0
        };
        (self.bytes_written, self.expected_size, percentage)
    }

    pub fn chunks_received(&self) -> usize {
        self.chunks_received.len()
    }

    pub fn pending_chunks(&self) -> usize {
        self.pending_chunks.len()
    }
}
