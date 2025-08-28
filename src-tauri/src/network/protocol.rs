use serde::{Deserialize, Serialize};
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

    // Pairing messages
    PairingRequest {
        device_id: Uuid,
        device_name: String,
        pin_hash: String,
        platform: Option<String>,
    },
    PairingChallenge {
        nonce: Vec<u8>,
        timestamp: u64,
    },
    PairingResponse {
        pin_hash: String,
        nonce: Vec<u8>,
        signature: Vec<u8>,
    },
    PairingResult {
        success: bool,
        device_id: Option<Uuid>,
        device_name: Option<String>,
        reason: Option<String>,
    },

    ClipboardUpdate {
        file_path: String,
        source_device: Uuid,
        timestamp: u64,
        file_size: u64,
    },
    ClipboardClear,

    // File request for paste operation
    FileRequest {
        request_id: Uuid,
        file_path: String,
        target_path: String,
    },
    FileRequestResponse {
        request_id: Uuid,
        accepted: bool,
        reason: Option<String>,
    },

    // HTTP-based file transfer
    FileOfferHttp {
        request_id: Uuid,
        filename: String,
        file_size: u64,
        download_url: String,
    },
    FileOfferHttpResponse {
        request_id: Uuid,
        accepted: bool,
        reason: Option<String>,
    },

    // Control
    Ping,
    Pong,
    Disconnect,
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

