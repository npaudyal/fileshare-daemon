use crate::{FileshareError, Result};
use crate::pairing::{
    crypto::{DeviceKeypair, create_pairing_challenge, verify_pairing_challenge},
    pin::PinGenerator,
    session::{PairingSession, PairingState},
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedDevice {
    pub device_id: Uuid,
    pub name: String,
    pub public_key: Vec<u8>,
    pub paired_at: u64,
    pub last_seen: u64,
    pub trust_level: TrustLevel,
    pub auto_accept_files: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TrustLevel {
    Verified,  // Successfully paired with PIN verification
    Trusted,   // Legacy or manually trusted
    Unknown,   // Not paired
}

pub struct PairingManager {
    device_id: Uuid,
    device_name: String,
    device_keypair: Arc<DeviceKeypair>,
    active_sessions: Arc<RwLock<HashMap<Uuid, PairingSession>>>,
    paired_devices: Arc<RwLock<HashMap<Uuid, PairedDevice>>>,
    pin_generator: Arc<PinGenerator>,
    last_cleanup: Arc<RwLock<Instant>>,
}

impl PairingManager {
    pub fn new(
        device_id: Uuid,
        device_name: String,
        keypair_path: PathBuf,
        paired_devices: Vec<PairedDevice>,
    ) -> Result<Self> {
        // Load or generate device keypair
        let device_keypair = Arc::new(DeviceKeypair::load_or_generate(&keypair_path)?);
        
        // Convert paired devices list to HashMap
        let mut devices_map = HashMap::new();
        for device in paired_devices {
            devices_map.insert(device.device_id, device);
        }

        Ok(Self {
            device_id,
            device_name,
            device_keypair,
            active_sessions: Arc::new(RwLock::new(HashMap::new())),
            paired_devices: Arc::new(RwLock::new(devices_map)),
            pin_generator: Arc::new(PinGenerator::new()),
            last_cleanup: Arc::new(RwLock::new(Instant::now())),
        })
    }

    /// Get the device's public key
    pub fn get_public_key(&self) -> Vec<u8> {
        self.device_keypair.public_key_bytes()
    }

    /// Update session state for a device
    pub async fn update_session_for_challenge(&self, device_id: Uuid, public_key: Vec<u8>, pin: String) -> Result<()> {
        let mut sessions = self.active_sessions.write().await;
        if let Some(session) = sessions.get_mut(&device_id) {
            session.peer_public_key = public_key;
            session.pin = pin;
            session.update_state(crate::pairing::PairingState::Challenging);
            Ok(())
        } else {
            Err(FileshareError::Pairing("Session not found".to_string()))
        }
    }

    /// Initiate pairing with another device
    pub async fn initiate_pairing(
        &self,
        peer_device_id: Uuid,
        peer_name: String,
    ) -> Result<PairingSession> {
        info!("Initiating pairing with device: {} ({})", peer_name, peer_device_id);

        // Check if already paired
        {
            let paired = self.paired_devices.read().await;
            if paired.contains_key(&peer_device_id) {
                return Err(FileshareError::Pairing("Device is already paired".to_string()));
            }
        }

        // Check for existing session
        {
            let sessions = self.active_sessions.read().await;
            if let Some(existing) = sessions.get(&peer_device_id) {
                if !existing.is_terminal() {
                    return Err(FileshareError::Pairing("Pairing already in progress".to_string()));
                }
            }
        }

        // Generate PIN for this session
        let pin = self.pin_generator.generate_pin().await;
        
        // Create new session
        let session = PairingSession::new(
            peer_device_id,
            peer_name,
            vec![], // Will be filled when we receive challenge
            pin.clone(),
            true, // Initiated by us
        );

        // Store session
        {
            let mut sessions = self.active_sessions.write().await;
            sessions.insert(peer_device_id, session.clone());
        }

        // Clean up old sessions
        self.cleanup_expired_sessions().await;

        Ok(session)
    }

    /// Handle incoming pairing request
    pub async fn handle_pairing_request(
        &self,
        peer_device_id: Uuid,
        peer_name: String,
        peer_public_key: Vec<u8>,
    ) -> Result<PairingSession> {
        info!("Received pairing request from: {} ({})", peer_name, peer_device_id);

        // Check if already paired
        {
            let paired = self.paired_devices.read().await;
            if paired.contains_key(&peer_device_id) {
                return Err(FileshareError::Pairing("Device is already paired".to_string()));
            }
        }

        // Generate PIN for this session
        let pin = self.pin_generator.generate_pin().await;
        
        // Create new session (not initiated by us)
        let session = PairingSession::new(
            peer_device_id,
            peer_name,
            peer_public_key,
            pin.clone(),
            false, // Not initiated by us
        );

        // Store session
        {
            let mut sessions = self.active_sessions.write().await;
            sessions.insert(peer_device_id, session.clone());
        }

        Ok(session)
    }

    /// Confirm pairing (user approved the PIN)
    pub async fn confirm_pairing(&self, session_id: Uuid) -> Result<Vec<u8>> {
        let mut sessions = self.active_sessions.write().await;
        
        // Find session by ID
        let peer_device_id = sessions
            .iter()
            .find(|(_, s)| s.session_id == session_id)
            .map(|(id, _)| *id)
            .ok_or_else(|| FileshareError::Pairing("Session not found".to_string()))?;

        let session = sessions
            .get_mut(&peer_device_id)
            .ok_or_else(|| FileshareError::Pairing("Session not found".to_string()))?;

        // Confirm the session
        session.confirm()
            .map_err(|e| FileshareError::Pairing(e))?;

        // Create signed challenge
        let signature = create_pairing_challenge(
            &session.pin,
            &self.device_id.to_string(),
            &self.device_keypair,
        );

        info!("Pairing confirmed for session: {}", session_id);
        Ok(signature)
    }

    /// Verify pairing confirmation from peer
    pub async fn verify_pairing_confirmation(
        &self,
        peer_device_id: Uuid,
        signature: Vec<u8>,
    ) -> Result<()> {
        let mut sessions = self.active_sessions.write().await;
        
        let session = sessions
            .get_mut(&peer_device_id)
            .ok_or_else(|| FileshareError::Pairing("Session not found".to_string()))?;

        // Verify the signature
        let verified = verify_pairing_challenge(
            &session.pin,
            &peer_device_id.to_string(),
            &signature,
            &session.peer_public_key,
        );

        if !verified {
            session.state = PairingState::Failed("Invalid signature".to_string());
            return Err(FileshareError::Pairing("Signature verification failed".to_string()));
        }

        // Mark as confirmed
        session.state = PairingState::Confirmed;
        info!("Pairing confirmation verified for device: {}", peer_device_id);

        Ok(())
    }

    /// Complete pairing and store the paired device
    pub async fn complete_pairing(&self, peer_device_id: Uuid) -> Result<PairedDevice> {
        let session = {
            let mut sessions = self.active_sessions.write().await;

            let session = sessions
                .get_mut(&peer_device_id)
                .ok_or_else(|| FileshareError::Pairing("Session not found".to_string()))?;

            // Accept both AwaitingConfirm (device that confirmed first) and Confirmed (device that received confirmation)
            // This makes pairing work regardless of which device confirms first
            if session.state != PairingState::Confirmed && session.state != PairingState::AwaitingConfirm {
                return Err(FileshareError::Pairing(format!("Invalid session state for completion: {:?}", session.state)));
            }

            session.complete();
            session.clone()
        };

        // Create paired device entry
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let paired_device = PairedDevice {
            device_id: peer_device_id,
            name: session.peer_name.clone(),
            public_key: session.peer_public_key.clone(),
            paired_at: now,
            last_seen: now,
            trust_level: TrustLevel::Verified,
            auto_accept_files: false,
        };

        // Store paired device
        {
            let mut paired = self.paired_devices.write().await;
            paired.insert(peer_device_id, paired_device.clone());
        }

        info!("Pairing completed successfully with: {} ({})", 
              session.peer_name, peer_device_id);

        // Remove session
        {
            let mut sessions = self.active_sessions.write().await;
            sessions.remove(&peer_device_id);
        }

        Ok(paired_device)
    }


    /// Reject pairing
    pub async fn reject_pairing(&self, session_id: Uuid, reason: String) -> Result<()> {
        let mut sessions = self.active_sessions.write().await;
        
        // Find and update session
        for session in sessions.values_mut() {
            if session.session_id == session_id {
                session.reject(reason.clone());
                info!("Pairing rejected for session: {} - {}", session_id, reason);
                return Ok(());
            }
        }

        Err(FileshareError::Pairing("Session not found".to_string()))
    }

    /// Unpair a device
    pub async fn unpair_device(&self, device_id: Uuid) -> Result<()> {
        let mut paired = self.paired_devices.write().await;
        
        if paired.remove(&device_id).is_some() {
            info!("Device unpaired: {}", device_id);
            Ok(())
        } else {
            Err(FileshareError::Pairing("Device not paired".to_string()))
        }
    }

    /// Check if a device is paired
    pub async fn is_device_paired(&self, device_id: Uuid) -> bool {
        let paired = self.paired_devices.read().await;
        paired.contains_key(&device_id)
    }

    /// Get paired device info
    pub async fn get_paired_device(&self, device_id: Uuid) -> Option<PairedDevice> {
        let paired = self.paired_devices.read().await;
        paired.get(&device_id).cloned()
    }

    /// Get all paired devices
    pub async fn get_all_paired_devices(&self) -> Vec<PairedDevice> {
        let paired = self.paired_devices.read().await;
        paired.values().cloned().collect()
    }

    /// Get active pairing sessions
    pub async fn get_active_sessions(&self) -> Vec<PairingSession> {
        let sessions = self.active_sessions.read().await;
        sessions.values().cloned().collect()
    }

    /// Get device public key
    pub fn get_device_public_key(&self) -> Vec<u8> {
        self.device_keypair.public_key_bytes()
    }

    /// Update last seen time for a paired device
    pub async fn update_last_seen(&self, device_id: Uuid) {
        let mut paired = self.paired_devices.write().await;
        
        if let Some(device) = paired.get_mut(&device_id) {
            device.last_seen = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();
        }
    }

    /// Clean up expired sessions
    async fn cleanup_expired_sessions(&self) {
        let mut last_cleanup = self.last_cleanup.write().await;
        
        // Only cleanup every 30 seconds
        if last_cleanup.elapsed() < Duration::from_secs(30) {
            return;
        }

        *last_cleanup = Instant::now();
        drop(last_cleanup); // Release the lock

        let mut sessions = self.active_sessions.write().await;
        let expired: Vec<Uuid> = sessions
            .iter()
            .filter(|(_, session)| session.is_expired())
            .map(|(id, _)| *id)
            .collect();

        for id in expired {
            if let Some(mut session) = sessions.remove(&id) {
                session.timeout();
                warn!("Pairing session expired for device: {}", id);
            }
        }
    }

    /// Sign a message with device private key
    pub fn sign_message(&self, message: &[u8]) -> Vec<u8> {
        self.device_keypair.sign(message)
    }

    /// Verify a message signature from a paired device
    pub async fn verify_message(
        &self,
        device_id: Uuid,
        message: &[u8],
        signature: &[u8],
    ) -> bool {
        let paired = self.paired_devices.read().await;
        
        if let Some(device) = paired.get(&device_id) {
            DeviceKeypair::verify_from_public_key(&device.public_key, message, signature)
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_pairing_flow() {
        let temp_dir = tempdir().unwrap();
        let keypair_path = temp_dir.path().join("device.key");

        let manager = PairingManager::new(
            Uuid::new_v4(),
            "Test Device".to_string(),
            keypair_path,
            vec![],
        ).unwrap();

        let peer_id = Uuid::new_v4();
        let session = manager.initiate_pairing(
            peer_id,
            "Peer Device".to_string(),
        ).await.unwrap();

        assert_eq!(session.state, PairingState::Initiated);
        assert!(session.initiated_by_us);
    }

    #[tokio::test]
    async fn test_duplicate_pairing() {
        let temp_dir = tempdir().unwrap();
        let keypair_path = temp_dir.path().join("device.key");

        let manager = PairingManager::new(
            Uuid::new_v4(),
            "Test Device".to_string(),
            keypair_path,
            vec![],
        ).unwrap();

        let peer_id = Uuid::new_v4();
        
        // First pairing should succeed
        manager.initiate_pairing(peer_id, "Peer".to_string()).await.unwrap();
        
        // Second pairing should fail
        let result = manager.initiate_pairing(peer_id, "Peer".to_string()).await;
        assert!(result.is_err());
    }
}