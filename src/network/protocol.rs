use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: Uuid,
    pub message_type: MessageType,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MessageType {
    // Authentication
    Handshake {
        device_id: Uuid,
        device_name: String,
        version: String,
    },
    HandshakeResponse {
        accepted: bool,
        reason: Option<String>,
    },

    // File transfer
    FileOffer {
        transfer_id: Uuid,
        metadata: FileMetadata,
    },
    FileOfferResponse {
        transfer_id: Uuid,
        accepted: bool,
        reason: Option<String>,
    },
    FileChunk {
        transfer_id: Uuid,
        chunk: TransferChunk,
    },
    FileChunkAck {
        transfer_id: Uuid,
        chunk_index: u64,
    },
    TransferComplete {
        transfer_id: Uuid,
        checksum: String,
    },
    TransferError {
        transfer_id: Uuid,
        error: String,
    },

    // Control
    Ping,
    Pong,
    Disconnect,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMetadata {
    pub name: String,
    pub size: u64,
    pub checksum: String,
    pub mime_type: Option<String>,
    pub created: Option<u64>,
    pub modified: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferChunk {
    pub index: u64,
    pub data: Vec<u8>,
    pub is_last: bool,
}

impl Message {
    pub fn new(message_type: MessageType) -> Self {
        Self {
            id: Uuid::new_v4(),
            message_type,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        }
    }

    pub fn handshake(device_id: Uuid, device_name: String) -> Self {
        Self::new(MessageType::Handshake {
            device_id,
            device_name,
            version: env!("CARGO_PKG_VERSION").to_string(),
        })
    }

    pub fn ping() -> Self {
        Self::new(MessageType::Ping)
    }

    pub fn pong() -> Self {
        Self::new(MessageType::Pong)
    }
}

impl FileMetadata {
    pub fn from_path(path: &PathBuf) -> crate::Result<Self> {
        use sha2::{Digest, Sha256};
        use std::fs;
        use std::io::Read;

        let metadata = fs::metadata(path)?;
        let name = path
            .file_name()
            .ok_or_else(|| crate::FileshareError::FileOperation("Invalid file name".to_string()))?
            .to_string_lossy()
            .to_string();

        // Calculate checksum
        let mut file = fs::File::open(path)?;
        let mut hasher = Sha256::new();
        let mut buffer = [0; 8192];

        loop {
            let bytes_read = file.read(&mut buffer)?;
            if bytes_read == 0 {
                break;
            }
            hasher.update(&buffer[..bytes_read]);
        }

        let checksum = format!("{:x}", hasher.finalize());

        // Get timestamps
        let created = metadata
            .created()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs());

        let modified = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs());

        // Guess MIME type
        let mime_type = Self::guess_mime_type(&name);

        Ok(Self {
            name,
            size: metadata.len(),
            checksum,
            mime_type,
            created,
            modified,
        })
    }

    fn guess_mime_type(filename: &str) -> Option<String> {
        let extension = std::path::Path::new(filename)
            .extension()?
            .to_str()?
            .to_lowercase();

        match extension.as_str() {
            "txt" => Some("text/plain".to_string()),
            "pdf" => Some("application/pdf".to_string()),
            "jpg" | "jpeg" => Some("image/jpeg".to_string()),
            "png" => Some("image/png".to_string()),
            "gif" => Some("image/gif".to_string()),
            "mp4" => Some("video/mp4".to_string()),
            "mp3" => Some("audio/mpeg".to_string()),
            "zip" => Some("application/zip".to_string()),
            "json" => Some("application/json".to_string()),
            "xml" => Some("application/xml".to_string()),
            _ => None,
        }
    }
}
