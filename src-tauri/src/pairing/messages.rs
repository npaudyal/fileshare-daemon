use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingMessage {
    pub id: Uuid,
    pub session_id: Uuid,
    pub message_type: PairingMessageType,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PairingMessageType {
    /// Sent by device wanting to pair
    PairingRequest {
        ephemeral_public_key: Vec<u8>,  // X25519 public key
        device_info: DeviceInfo,
    },
    
    /// Sent by device showing PIN
    PairingChallenge {
        ephemeral_public_key: Vec<u8>,
        encrypted_challenge: Vec<u8>,  // AES-256-GCM encrypted
        nonce: Vec<u8>,
    },
    
    /// Response with PIN attempt
    PairingResponse {
        pin: String,
        encrypted_response: Vec<u8>,
        device_public_key: Vec<u8>,  // Ed25519 long-term key
    },
    
    /// Final confirmation
    PairingComplete {
        device_public_key: Vec<u8>,
        encrypted_confirmation: Vec<u8>,
        signature: Vec<u8>,  // Ed25519 signature
    },
    
    /// Reject pairing
    PairingRejected {
        reason: String,
    },
    
    /// Cancel pairing
    PairingCancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub id: Uuid,
    pub name: String,
    pub device_type: String,
    pub platform: String,
    pub version: String,
}

impl PairingMessage {
    pub fn new(session_id: Uuid, message_type: PairingMessageType) -> Self {
        Self {
            id: Uuid::new_v4(),
            session_id,
            message_type,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        }
    }
    
    pub fn pairing_request(
        session_id: Uuid,
        ephemeral_public_key: Vec<u8>,
        device_info: DeviceInfo,
    ) -> Self {
        Self::new(
            session_id,
            PairingMessageType::PairingRequest {
                ephemeral_public_key,
                device_info,
            },
        )
    }
    
    pub fn pairing_challenge(
        session_id: Uuid,
        ephemeral_public_key: Vec<u8>,
        encrypted_challenge: Vec<u8>,
        nonce: Vec<u8>,
    ) -> Self {
        Self::new(
            session_id,
            PairingMessageType::PairingChallenge {
                ephemeral_public_key,
                encrypted_challenge,
                nonce,
            },
        )
    }
    
    pub fn pairing_response(
        session_id: Uuid,
        pin: String,
        encrypted_response: Vec<u8>,
        device_public_key: Vec<u8>,
    ) -> Self {
        Self::new(
            session_id,
            PairingMessageType::PairingResponse {
                pin,
                encrypted_response,
                device_public_key,
            },
        )
    }
    
    pub fn pairing_complete(
        session_id: Uuid,
        device_public_key: Vec<u8>,
        encrypted_confirmation: Vec<u8>,
        signature: Vec<u8>,
    ) -> Self {
        Self::new(
            session_id,
            PairingMessageType::PairingComplete {
                device_public_key,
                encrypted_confirmation,
                signature,
            },
        )
    }
    
    pub fn pairing_rejected(session_id: Uuid, reason: String) -> Self {
        Self::new(session_id, PairingMessageType::PairingRejected { reason })
    }
    
    pub fn pairing_cancelled(session_id: Uuid) -> Self {
        Self::new(session_id, PairingMessageType::PairingCancelled)
    }
}