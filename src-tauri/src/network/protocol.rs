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

    // Pairing messages
    PairingRequest {
        device_id: Uuid,
        device_name: String,
        public_key: Vec<u8>,
    },
    PairingChallenge {
        device_id: Uuid,
        device_name: String,
        public_key: Vec<u8>,
        pin: String,
        session_id: Uuid,
    },
    PairingConfirm {
        session_id: Uuid,
        signed_challenge: Vec<u8>,
    },
    PairingComplete {
        session_id: Uuid,
        signed_acknowledgment: Vec<u8>,
    },
    PairingReject {
        session_id: Uuid,
        reason: String,
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

