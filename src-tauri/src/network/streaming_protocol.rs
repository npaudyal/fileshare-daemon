use crc32fast::Hasher;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// Lean binary protocol header - only 32 bytes total
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct StreamChunkHeader {
    pub transfer_id: [u8; 16], // UUID as raw bytes
    pub chunk_index: u64,      // 8 bytes
    pub chunk_size: u32,       // 4 bytes - actual data size
    pub flags: u8,             // 1 byte - compressed, last, etc.
    pub checksum: u32,         // 4 bytes - CRC32
    pub reserved: [u8; 3],     // 3 bytes - for future use
}

// Flags for the header
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
            reserved: [0; 3],
        }
    }

    pub fn set_compressed(&mut self) {
        self.flags |= FLAG_COMPRESSED;
    }

    pub fn set_last_chunk(&mut self) {
        self.flags |= FLAG_LAST_CHUNK;
    }

    pub fn is_compressed(&self) -> bool {
        self.flags & FLAG_COMPRESSED != 0
    }

    pub fn is_last_chunk(&self) -> bool {
        self.flags & FLAG_LAST_CHUNK != 0
    }

    pub fn calculate_checksum(&mut self, data: &[u8]) {
        let mut hasher = Hasher::new();
        hasher.update(data);
        self.checksum = hasher.finalize();
    }

    pub fn verify_checksum(&self, data: &[u8]) -> bool {
        let mut hasher = Hasher::new();
        hasher.update(data);
        hasher.finalize() == self.checksum
    }

    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        unsafe { std::mem::transmute(*self) }
    }

    pub fn from_bytes(bytes: &[u8; Self::SIZE]) -> Self {
        unsafe { std::mem::transmute(*bytes) }
    }

    pub fn transfer_id(&self) -> Uuid {
        Uuid::from_bytes(self.transfer_id)
    }

    // FIXED: Safe field access for packed struct
    pub fn get_chunk_index(&self) -> u64 {
        // Copy the field value to avoid unaligned reference
        let chunk_index = self.chunk_index;
        chunk_index
    }

    pub fn get_chunk_size(&self) -> u32 {
        let chunk_size = self.chunk_size;
        chunk_size
    }

    pub fn get_flags(&self) -> u8 {
        let flags = self.flags;
        flags
    }

    pub fn get_checksum(&self) -> u32 {
        let checksum = self.checksum;
        checksum
    }
}

// FIXED: Add Clone and Copy derives to StreamingConfig
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct StreamingConfig {
    pub base_chunk_size: usize,
    pub max_chunk_size: usize,
    pub compression_threshold: usize, // Don't compress chunks smaller than this
    pub memory_limit: usize,          // Max memory to use for buffers
    pub enable_compression: bool,
    pub compression_level: i32,
}

impl Default for StreamingConfig {
    fn default() -> Self {
        Self {
            base_chunk_size: 256 * 1024,     // 256KB default (up from 64KB)
            max_chunk_size: 2 * 1024 * 1024, // 2MB max chunks
            compression_threshold: 1024,     // Don't compress < 1KB
            memory_limit: 50 * 1024 * 1024,  // 50MB memory limit
            enable_compression: true,
            compression_level: 1, // Fast compression
        }
    }
}

// Enhanced metadata for streaming transfers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamingFileMetadata {
    pub name: String,
    pub size: u64,
    pub modified: Option<u64>,
    pub mime_type: Option<String>,
    pub target_dir: Option<String>,

    // Streaming-specific metadata
    pub suggested_chunk_size: usize,
    pub supports_compression: bool,
    pub estimated_chunks: u64,
    pub file_hash: String, // Still SHA256 for file integrity
}
