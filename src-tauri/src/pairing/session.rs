use crate::pairing::{crypto::PairingCrypto, errors::PairingError, messages::DeviceInfo};
use ring::agreement::EphemeralPrivateKey;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};
use uuid::Uuid;

const SESSION_TIMEOUT_SECONDS: u64 = 60;
const MAX_PIN_ATTEMPTS: u32 = 3;

#[derive(Debug, Clone)]
pub enum PairingRole {
    Initiator,  // Device that shows the PIN
    Acceptor,   // Device that enters the PIN
}

#[derive(Debug, Clone)]
pub enum PairingState {
    WaitingForRequest,
    WaitingForChallenge,
    WaitingForResponse,
    WaitingForConfirmation,
    Completed,
    Failed(String),
    Cancelled,
}

#[derive(Clone)]
pub struct PairingSession {
    pub id: Uuid,
    pub role: PairingRole,
    pub state: PairingState,
    pub peer_device: Option<DeviceInfo>,
    pub pin: Option<String>,
    pub pin_attempts: u32,
    pub ephemeral_private_key: Option<Vec<u8>>,  // Store as bytes
    pub ephemeral_public_key: Option<Vec<u8>>,
    pub peer_ephemeral_public_key: Option<Vec<u8>>,
    pub shared_secret: Option<Vec<u8>>,
    pub device_private_key: Option<Vec<u8>>,
    pub device_public_key: Option<Vec<u8>>,
    pub peer_device_public_key: Option<Vec<u8>>,
    pub created_at: Instant,
    pub last_activity: Instant,
}

impl PairingSession {
    pub fn new_initiator() -> Self {
        let pin = PairingCrypto::generate_pin();
        info!("Created new pairing session as initiator with PIN: {}", pin);
        
        Self {
            id: Uuid::new_v4(),
            role: PairingRole::Initiator,
            state: PairingState::WaitingForRequest,
            peer_device: None,
            pin: Some(pin),
            pin_attempts: 0,
            ephemeral_private_key: None,
            ephemeral_public_key: None,
            peer_ephemeral_public_key: None,
            shared_secret: None,
            device_private_key: None,
            device_public_key: None,
            peer_device_public_key: None,
            created_at: Instant::now(),
            last_activity: Instant::now(),
        }
    }
    
    pub fn new_acceptor(peer_device: DeviceInfo) -> Self {
        info!("Created new pairing session as acceptor for device: {}", peer_device.name);
        
        Self {
            id: Uuid::new_v4(),
            role: PairingRole::Acceptor,
            state: PairingState::WaitingForChallenge,
            peer_device: Some(peer_device),
            pin: None,
            pin_attempts: 0,
            ephemeral_private_key: None,
            ephemeral_public_key: None,
            peer_ephemeral_public_key: None,
            shared_secret: None,
            device_private_key: None,
            device_public_key: None,
            peer_device_public_key: None,
            created_at: Instant::now(),
            last_activity: Instant::now(),
        }
    }
    
    pub fn is_expired(&self) -> bool {
        self.created_at.elapsed() > Duration::from_secs(SESSION_TIMEOUT_SECONDS)
    }
    
    pub fn is_completed(&self) -> bool {
        matches!(self.state, PairingState::Completed)
    }
    
    pub fn is_failed(&self) -> bool {
        matches!(self.state, PairingState::Failed(_))
    }
    
    pub fn update_activity(&mut self) {
        self.last_activity = Instant::now();
    }
    
    pub fn verify_pin(&mut self, provided_pin: &str) -> Result<bool, PairingError> {
        if self.pin_attempts >= MAX_PIN_ATTEMPTS {
            self.state = PairingState::Failed("Maximum PIN attempts exceeded".to_string());
            return Err(PairingError::MaxAttemptsExceeded);
        }
        
        self.pin_attempts += 1;
        self.update_activity();
        
        if let Some(ref expected_pin) = self.pin {
            let is_valid = PairingCrypto::verify_pin(provided_pin, expected_pin);
            if !is_valid && self.pin_attempts >= MAX_PIN_ATTEMPTS {
                self.state = PairingState::Failed("Maximum PIN attempts exceeded".to_string());
                return Err(PairingError::MaxAttemptsExceeded);
            }
            Ok(is_valid)
        } else {
            Err(PairingError::InvalidStateTransition)
        }
    }
}

pub struct PairingSessionManager {
    sessions: Arc<RwLock<HashMap<Uuid, PairingSession>>>,
}

impl PairingSessionManager {
    pub fn new() -> Self {
        let manager = Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
        };
        
        // Start cleanup task
        let sessions = manager.sessions.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(10)).await;
                Self::cleanup_expired_sessions(sessions.clone()).await;
            }
        });
        
        manager
    }
    
    pub async fn create_initiator_session(&self) -> Result<(Uuid, String), PairingError> {
        let session = PairingSession::new_initiator();
        let session_id = session.id;
        let pin = session.pin.clone().unwrap();
        
        let mut sessions = self.sessions.write().await;
        sessions.insert(session_id, session);
        
        info!("Created initiator session {} with PIN {}", session_id, pin);
        Ok((session_id, pin))
    }
    
    pub async fn create_acceptor_session(&self, peer_device: DeviceInfo) -> Result<Uuid, PairingError> {
        let session = PairingSession::new_acceptor(peer_device);
        let session_id = session.id;
        
        let mut sessions = self.sessions.write().await;
        sessions.insert(session_id, session);
        
        info!("Created acceptor session {}", session_id);
        Ok(session_id)
    }
    
    pub async fn create_acceptor_session_with_id(&self, session_id: Uuid, peer_device: DeviceInfo) -> Result<(), PairingError> {
        let mut session = PairingSession::new_acceptor(peer_device);
        session.id = session_id; // Use the provided session ID
        
        let mut sessions = self.sessions.write().await;
        sessions.insert(session_id, session);
        
        info!("Created acceptor session with specific ID: {}", session_id);
        Ok(())
    }
    
    pub async fn get_session(&self, session_id: Uuid) -> Result<PairingSession, PairingError> {
        let sessions = self.sessions.read().await;
        sessions
            .get(&session_id)
            .cloned()
            .ok_or(PairingError::SessionNotFound)
    }
    
    pub async fn update_session<F>(&self, session_id: Uuid, updater: F) -> Result<(), PairingError>
    where
        F: FnOnce(&mut PairingSession) -> Result<(), PairingError>,
    {
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(&session_id)
            .ok_or(PairingError::SessionNotFound)?;
        
        if session.is_expired() {
            return Err(PairingError::SessionExpired);
        }
        
        session.update_activity();
        updater(session)
    }
    
    pub async fn verify_session_pin(&self, session_id: Uuid, pin: &str) -> Result<bool, PairingError> {
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(&session_id)
            .ok_or(PairingError::SessionNotFound)?;
        
        if session.is_expired() {
            return Err(PairingError::SessionExpired);
        }
        
        session.verify_pin(pin)
    }
    
    pub async fn complete_session(&self, session_id: Uuid) -> Result<(), PairingError> {
        self.update_session(session_id, |session| {
            session.state = PairingState::Completed;
            info!("Pairing session {} completed successfully", session_id);
            Ok(())
        }).await
    }
    
    pub async fn cancel_session(&self, session_id: Uuid) -> Result<(), PairingError> {
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get_mut(&session_id) {
            session.state = PairingState::Cancelled;
            info!("Pairing session {} cancelled", session_id);
        }
        Ok(())
    }
    
    pub async fn remove_session(&self, session_id: Uuid) {
        let mut sessions = self.sessions.write().await;
        if sessions.remove(&session_id).is_some() {
            debug!("Removed pairing session {}", session_id);
        }
    }
    
    pub async fn get_active_sessions(&self) -> Vec<(Uuid, PairingRole, PairingState)> {
        let sessions = self.sessions.read().await;
        sessions
            .values()
            .filter(|s| !s.is_expired() && !s.is_completed() && !s.is_failed())
            .map(|s| (s.id, s.role.clone(), s.state.clone()))
            .collect()
    }
    
    async fn cleanup_expired_sessions(sessions: Arc<RwLock<HashMap<Uuid, PairingSession>>>) {
        let mut sessions = sessions.write().await;
        let expired: Vec<Uuid> = sessions
            .iter()
            .filter(|(_, session)| session.is_expired() || session.is_completed() || session.is_failed())
            .map(|(id, _)| *id)
            .collect();
        
        for id in expired {
            sessions.remove(&id);
            debug!("Cleaned up expired/completed pairing session {}", id);
        }
    }
}