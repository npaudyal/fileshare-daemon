use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingSession {
    pub session_id: Uuid,
    pub peer_device_id: Uuid,
    pub peer_name: String,
    pub peer_public_key: Vec<u8>,
    pub pin: String,
    pub state: PairingState,
    pub initiated_by_us: bool,
    pub created_at: u64,  // Unix timestamp
    pub expires_at: u64,  // Unix timestamp
    #[serde(skip)]
    pub created_instant: Option<Instant>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PairingState {
    Initiated,        // Request sent, waiting for challenge
    Challenging,      // PIN displayed, waiting for user confirmation  
    AwaitingConfirm,  // User confirmed, waiting for peer
    Confirmed,        // Both confirmed, completing pairing
    Completed,        // Successfully paired
    Failed(String),   // Pairing failed
    Timeout,          // Session timed out
}

impl PairingSession {
    pub fn new(
        peer_device_id: Uuid,
        peer_name: String,
        peer_public_key: Vec<u8>,
        pin: String,
        initiated_by_us: bool,
    ) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        
        Self {
            session_id: Uuid::new_v4(),
            peer_device_id,
            peer_name,
            peer_public_key,
            pin,
            state: if initiated_by_us {
                PairingState::Initiated
            } else {
                PairingState::Challenging
            },
            initiated_by_us,
            created_at: now,
            expires_at: now + 300, // 5 minutes timeout
            created_instant: Some(Instant::now()),
        }
    }

    /// Check if session has expired
    pub fn is_expired(&self) -> bool {
        if let Some(created) = self.created_instant {
            created.elapsed() > Duration::from_secs(300)
        } else {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();
            now > self.expires_at
        }
    }

    /// Get remaining time in seconds
    pub fn remaining_seconds(&self) -> u64 {
        if self.is_expired() {
            return 0;
        }

        if let Some(created) = self.created_instant {
            let elapsed = created.elapsed().as_secs();
            if elapsed < 300 {
                300 - elapsed
            } else {
                0
            }
        } else {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();
            
            if now < self.expires_at {
                self.expires_at - now
            } else {
                0
            }
        }
    }

    /// Update session state
    pub fn update_state(&mut self, new_state: PairingState) {
        self.state = new_state;
    }

    /// Check if session is in a terminal state
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.state,
            PairingState::Completed | PairingState::Failed(_) | PairingState::Timeout
        )
    }

    /// Check if session is waiting for user action
    pub fn is_waiting_for_user(&self) -> bool {
        matches!(self.state, PairingState::Challenging)
    }

    /// Check if session can be confirmed
    pub fn can_confirm(&self) -> bool {
        matches!(self.state, PairingState::Challenging) && !self.is_expired()
    }

    /// Mark session as confirmed by user
    pub fn confirm(&mut self) -> Result<(), String> {
        if !self.can_confirm() {
            return Err("Cannot confirm session in current state".to_string());
        }

        self.state = PairingState::AwaitingConfirm;
        Ok(())
    }

    /// Mark session as rejected by user
    pub fn reject(&mut self, reason: String) {
        self.state = PairingState::Failed(reason);
    }

    /// Complete the pairing
    pub fn complete(&mut self) {
        self.state = PairingState::Completed;
    }

    /// Mark session as timed out
    pub fn timeout(&mut self) {
        if !self.is_terminal() {
            self.state = PairingState::Timeout;
        }
    }

    /// Get a display-friendly status message
    pub fn status_message(&self) -> String {
        match &self.state {
            PairingState::Initiated => "Waiting for response...".to_string(),
            PairingState::Challenging => format!("Verify PIN: {}", self.pin),
            PairingState::AwaitingConfirm => "Waiting for other device...".to_string(),
            PairingState::Confirmed => "Completing pairing...".to_string(),
            PairingState::Completed => "Successfully paired!".to_string(),
            PairingState::Failed(reason) => format!("Failed: {}", reason),
            PairingState::Timeout => "Session timed out".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_creation() {
        let session = PairingSession::new(
            Uuid::new_v4(),
            "Test Device".to_string(),
            vec![1, 2, 3],
            "1234".to_string(),
            true,
        );

        assert_eq!(session.state, PairingState::Initiated);
        assert!(session.initiated_by_us);
        assert!(!session.is_expired());
    }

    #[test]
    fn test_session_states() {
        let mut session = PairingSession::new(
            Uuid::new_v4(),
            "Test Device".to_string(),
            vec![1, 2, 3],
            "1234".to_string(),
            false,
        );

        assert_eq!(session.state, PairingState::Challenging);
        assert!(session.can_confirm());

        session.confirm().unwrap();
        assert_eq!(session.state, PairingState::AwaitingConfirm);
        assert!(!session.can_confirm());

        session.complete();
        assert_eq!(session.state, PairingState::Completed);
        assert!(session.is_terminal());
    }

    #[test]
    fn test_session_timeout() {
        let mut session = PairingSession::new(
            Uuid::new_v4(),
            "Test Device".to_string(),
            vec![1, 2, 3],
            "1234".to_string(),
            true,
        );

        // Manually set expired time
        session.expires_at = 0;
        assert!(session.is_expired());
        assert_eq!(session.remaining_seconds(), 0);

        session.timeout();
        assert_eq!(session.state, PairingState::Timeout);
    }
}