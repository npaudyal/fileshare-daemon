use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// High-performance QUIC-based file transfer protocol
/// Designed for 50-100 MBPS throughput with true parallelism

// Stream types for QUIC multiplexing
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamType {
    Control = 0,      // Control messages, handshake
    DataStream1 = 1,  // File chunk streams
    DataStream2 = 2,
    DataStream3 = 3,
    DataStream4 = 4,
    DataStream5 = 5,
    DataStream6 = 6,
    DataStream7 = 7,
    DataStream8 = 8,
    Progress = 9,     // Progress updates, flow control
}

impl StreamType {
    pub fn is_data_stream(&self) -> bool {
        matches!(self, 
            StreamType::DataStream1 | StreamType::DataStream2 | StreamType::DataStream3 |
            StreamType::DataStream4 | StreamType::DataStream5 | StreamType::DataStream6 |
            StreamType::DataStream7 | StreamType::DataStream8
        )
    }

    pub fn data_stream_id(&self) -> Option<u8> {
        match self {
            StreamType::DataStream1 => Some(1),
            StreamType::DataStream2 => Some(2),
            StreamType::DataStream3 => Some(3),
            StreamType::DataStream4 => Some(4),
            StreamType::DataStream5 => Some(5),
            StreamType::DataStream6 => Some(6),
            StreamType::DataStream7 => Some(7),
            StreamType::DataStream8 => Some(8),
            _ => None,
        }
    }

    pub fn from_stream_id(id: u64) -> Option<Self> {
        match id {
            0 => Some(StreamType::Control),
            1 => Some(StreamType::DataStream1),
            2 => Some(StreamType::DataStream2),
            3 => Some(StreamType::DataStream3),
            4 => Some(StreamType::DataStream4),
            5 => Some(StreamType::DataStream5),
            6 => Some(StreamType::DataStream6),
            7 => Some(StreamType::DataStream7),
            8 => Some(StreamType::DataStream8),
            9 => Some(StreamType::Progress),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QuicMessage {
    // Connection establishment
    Handshake {
        device_id: Uuid,
        device_name: String,
        version: String,
        capabilities: TransferCapabilities,
    },
    HandshakeResponse {
        accepted: bool,
        capabilities: TransferCapabilities,
        reason: Option<String>,
    },

    // File transfer initiation
    FileOffer {
        transfer_id: Uuid,
        metadata: QuicFileMetadata,
        stream_count: u8,
    },
    FileOfferResponse {
        transfer_id: Uuid,
        accepted: bool,
        reason: Option<String>,
    },

    // Transfer control
    TransferStart {
        transfer_id: Uuid,
        stream_assignments: HashMap<u8, u64>, // stream_id -> first_chunk
    },
    TransferPause {
        transfer_id: Uuid,
    },
    TransferResume {
        transfer_id: Uuid,
        resume_info: ResumeInfo,
    },
    TransferCancel {
        transfer_id: Uuid,
        reason: String,
    },
    TransferComplete {
        transfer_id: Uuid,
        checksum: String,
        total_bytes: u64,
    },

    // Flow control and congestion management
    FlowControl {
        transfer_id: Uuid,
        stream_id: u8,
        window_size: u64,
        congestion_level: CongestionLevel,
    },
    
    // Error handling
    TransferError {
        transfer_id: Uuid,
        error_type: TransferError,
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferCapabilities {
    pub max_streams: u8,
    pub max_chunk_size: usize,
    pub compression_support: Vec<CompressionType>,
    pub zero_copy_support: bool,
    pub resume_support: bool,
    pub bandwidth_limit: Option<u64>, // bytes per second
}

impl Default for TransferCapabilities {
    fn default() -> Self {
        Self {
            max_streams: 8,
            max_chunk_size: 1024 * 1024, // 1MB
            compression_support: vec![CompressionType::None, CompressionType::Zstd],
            zero_copy_support: true,
            resume_support: true,
            bandwidth_limit: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuicFileMetadata {
    pub name: String,
    pub size: u64,
    pub checksum: String,
    pub mime_type: Option<String>,
    pub created: Option<u64>,
    pub modified: Option<u64>,
    pub chunk_size: usize,
    pub total_chunks: u64,
    pub compression: CompressionType,
    pub target_dir: Option<String>,
    pub resume_token: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum CompressionType {
    None,
    Zstd,
    Lz4,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumeInfo {
    pub completed_chunks: Vec<u64>,
    pub partial_chunk: Option<PartialChunk>,
    pub bytes_transferred: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartialChunk {
    pub chunk_id: u64,
    pub bytes_received: usize,
    pub total_bytes: usize,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum CongestionLevel {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TransferError {
    StreamError(String),
    ChecksumMismatch,
    DiskFull,
    PermissionDenied,
    NetworkError(String),
    UnexpectedEof,
    CorruptedData,
}

// Raw chunk data transmitted over QUIC streams
#[derive(Debug)]
pub struct QuicChunk {
    pub transfer_id: Uuid,
    pub chunk_id: u64,
    pub stream_id: u8,
    pub data: Vec<u8>,
    pub is_last: bool,
    pub compressed: bool,
    pub chunk_checksum: Option<String>,
}

impl QuicChunk {
    pub fn new(transfer_id: Uuid, chunk_id: u64, stream_id: u8, data: Vec<u8>, is_last: bool) -> Self {
        Self {
            transfer_id,
            chunk_id,
            stream_id,
            data,
            is_last,
            compressed: false,
            chunk_checksum: None,
        }
    }

    pub fn with_compression(mut self, compressed: bool) -> Self {
        self.compressed = compressed;
        self
    }

    pub fn with_checksum(mut self, checksum: String) -> Self {
        self.chunk_checksum = Some(checksum);
        self
    }

    // Serialize chunk for transmission
    pub fn serialize(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(8 + 8 + 1 + 4 + self.data.len());
        
        // Header: transfer_id (16) + chunk_id (8) + stream_id (1) + data_len (4) + flags (1)
        bytes.extend_from_slice(self.transfer_id.as_bytes());
        bytes.extend_from_slice(&self.chunk_id.to_le_bytes());
        bytes.push(self.stream_id);
        bytes.extend_from_slice(&(self.data.len() as u32).to_le_bytes());
        
        let mut flags = 0u8;
        if self.is_last { flags |= 0x01; }
        if self.compressed { flags |= 0x02; }
        if self.chunk_checksum.is_some() { flags |= 0x04; }
        bytes.push(flags);
        
        // Checksum if present
        if let Some(ref checksum) = self.chunk_checksum {
            bytes.extend_from_slice(&(checksum.len() as u8).to_le_bytes());
            bytes.extend_from_slice(checksum.as_bytes());
        }
        
        // Chunk data
        bytes.extend_from_slice(&self.data);
        
        bytes
    }

    // Deserialize chunk from bytes
    pub fn deserialize(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() < 30 {
            return Err("Chunk too small".into());
        }

        let mut offset = 0;

        // Parse transfer_id
        let transfer_id = Uuid::from_bytes(
            bytes[offset..offset + 16].try_into().map_err(|e| format!("Invalid UUID: {}", e))?
        );
        offset += 16;

        // Parse chunk_id
        let chunk_id = u64::from_le_bytes(
            bytes[offset..offset + 8].try_into().map_err(|e| format!("Invalid chunk_id: {}", e))?
        );
        offset += 8;

        // Parse stream_id
        let stream_id = bytes[offset];
        offset += 1;

        // Parse data length
        let data_len = u32::from_le_bytes(
            bytes[offset..offset + 4].try_into().map_err(|e| format!("Invalid data_len: {}", e))?
        ) as usize;
        offset += 4;

        // Parse flags
        let flags = bytes[offset];
        offset += 1;

        let is_last = (flags & 0x01) != 0;
        let compressed = (flags & 0x02) != 0;
        let has_checksum = (flags & 0x04) != 0;

        // Parse checksum if present
        let chunk_checksum = if has_checksum {
            let checksum_len = bytes[offset] as usize;
            offset += 1;
            let checksum = String::from_utf8(
                bytes[offset..offset + checksum_len].to_vec()
            ).map_err(|e| format!("Invalid checksum UTF-8: {}", e))?;
            offset += checksum_len;
            Some(checksum)
        } else {
            None
        };

        // Parse data
        if bytes.len() < offset + data_len {
            return Err("Invalid chunk data length".into());
        }
        let data = bytes[offset..offset + data_len].to_vec();

        Ok(Self {
            transfer_id,
            chunk_id,
            stream_id,
            data,
            is_last,
            compressed,
            chunk_checksum,
        })
    }
}