use serde::{Deserialize, Serialize};
use uuid::Uuid;

// FIXED: Safe serialization with proper alignment
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct StreamChunkHeader {
    pub transfer_id: [u8; 16],
    pub chunk_index: u64,
    pub chunk_size: u32,
    pub flags: u8,
    pub checksum: u32,
}

// Header flags - moved to constants for safety
pub const FLAG_COMPRESSED: u8 = 0b00000001;
pub const FLAG_LAST_CHUNK: u8 = 0b00000010;
pub const FLAG_ENCRYPTED: u8 = 0b00000100;
pub const FLAG_PRIORITY: u8 = 0b00001000;

impl StreamChunkHeader {
    pub const SIZE: usize = std::mem::size_of::<Self>();

    pub fn new(transfer_id: Uuid, chunk_index: u64, chunk_size: u32) -> Self {
        Self {
            transfer_id: *transfer_id.as_bytes(),
            chunk_index,
            chunk_size,
            flags: 0,
            checksum: 0,
        }
    }

    // SAFE: Proper serialization
    pub fn to_bytes(&self) -> Result<Vec<u8>, bincode::Error> {
        bincode::serialize(self)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, bincode::Error> {
        bincode::deserialize(bytes)
    }

    // SAFE: Accessor methods
    pub fn get_transfer_id(&self) -> Uuid {
        Uuid::from_bytes(self.transfer_id)
    }

    pub fn get_chunk_index(&self) -> u64 {
        self.chunk_index
    }

    pub fn get_chunk_size(&self) -> u32 {
        self.chunk_size
    }

    pub fn get_flags(&self) -> u8 {
        self.flags
    }

    pub fn get_checksum(&self) -> u32 {
        self.checksum
    }

    // Flag manipulation methods
    pub fn set_compressed(&mut self) {
        self.flags |= FLAG_COMPRESSED;
    }

    pub fn set_last_chunk(&mut self) {
        self.flags |= FLAG_LAST_CHUNK;
    }

    pub fn set_encrypted(&mut self) {
        self.flags |= FLAG_ENCRYPTED;
    }

    pub fn set_priority(&mut self) {
        self.flags |= FLAG_PRIORITY;
    }

    pub fn is_compressed(&self) -> bool {
        self.flags & FLAG_COMPRESSED != 0
    }

    pub fn is_last_chunk(&self) -> bool {
        self.flags & FLAG_LAST_CHUNK != 0
    }

    pub fn is_encrypted(&self) -> bool {
        self.flags & FLAG_ENCRYPTED != 0
    }

    pub fn is_priority(&self) -> bool {
        self.flags & FLAG_PRIORITY != 0
    }

    // SAFE: CRC32 checksum calculation
    pub fn calculate_checksum(&mut self, data: &[u8]) {
        self.checksum = crc32fast::hash(data);
    }

    pub fn verify_checksum(&self, data: &[u8]) -> bool {
        crc32fast::hash(data) == self.checksum
    }
}

// Enhanced streaming configuration with validation
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct StreamingConfig {
    pub base_chunk_size: usize,
    pub max_chunk_size: usize,
    pub compression_threshold: usize,
    pub memory_limit: usize,
    pub enable_compression: bool,
    pub compression_level: i32,
    pub max_concurrent_chunks: usize,
    pub adaptive_chunk_sizing: bool,
    pub zero_copy_threshold: usize,
    pub backpressure_threshold: usize,
}

impl Default for StreamingConfig {
    fn default() -> Self {
        Self {
            base_chunk_size: 512 * 1024,     // 512KB
            max_chunk_size: 4 * 1024 * 1024, // 4MB
            compression_threshold: 4096,     // 4KB
            memory_limit: 100 * 1024 * 1024, // 100MB
            enable_compression: true,
            compression_level: 1,
            max_concurrent_chunks: 8,
            adaptive_chunk_sizing: true,
            zero_copy_threshold: 10 * 1024 * 1024,    // 10MB
            backpressure_threshold: 50 * 1024 * 1024, // 50MB
        }
    }
}

impl StreamingConfig {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.base_chunk_size == 0 {
            return Err("Base chunk size cannot be zero");
        }
        if self.max_chunk_size < self.base_chunk_size {
            return Err("Max chunk size must be >= base chunk size");
        }
        if self.memory_limit < self.max_chunk_size {
            return Err("Memory limit must be >= max chunk size");
        }
        if self.max_concurrent_chunks == 0 {
            return Err("Max concurrent chunks cannot be zero");
        }
        Ok(())
    }
}

// Enhanced metadata with better validation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamingFileMetadata {
    pub name: String,
    pub size: u64,
    pub modified: Option<u64>,
    pub mime_type: Option<String>,
    pub target_dir: Option<String>,
    pub suggested_chunk_size: usize,
    pub supports_compression: bool,
    pub estimated_chunks: u64,
    pub file_hash: String,
    pub transfer_id: Uuid,
    pub priority: u8,
    pub resume_supported: bool,
    pub encryption_enabled: bool,
    pub expected_transfer_time_secs: Option<u64>,
    pub compression_ratio_hint: Option<f32>,
}

impl StreamingFileMetadata {
    pub fn new(name: String, size: u64, chunk_size: usize) -> Self {
        let estimated_chunks = (size + chunk_size as u64 - 1) / chunk_size as u64;

        Self {
            name,
            size,
            modified: None,
            mime_type: None,
            target_dir: None,
            suggested_chunk_size: chunk_size,
            supports_compression: true,
            estimated_chunks,
            file_hash: String::new(),
            transfer_id: Uuid::new_v4(),
            priority: 128,
            resume_supported: size > 10 * 1024 * 1024,
            encryption_enabled: false,
            expected_transfer_time_secs: None,
            compression_ratio_hint: None,
        }
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.name.is_empty() {
            return Err("Empty filename");
        }
        if self.size == 0 {
            return Err("Zero file size");
        }
        if self.suggested_chunk_size == 0 {
            return Err("Invalid chunk size");
        }
        if self.estimated_chunks == 0 {
            return Err("Invalid chunk count");
        }
        // Validate filename for security
        if self
            .name
            .contains(['/', '\\', '<', '>', ':', '"', '|', '?', '*'])
        {
            return Err("Filename contains invalid characters");
        }
        Ok(())
    }
}
