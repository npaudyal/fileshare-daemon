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
    Initiated,        // Initiator: Request sent, waiting for challenge
    DisplayingPin,    // Initiator: Displaying PIN, waiting for target confirmation
    AwaitingApproval, // Target: Received request, showing PIN for user to approve/reject
    Confirmed,        // Target confirmed, completing pairing
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
                PairingState::AwaitingApproval
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
        matches!(self.state, PairingState::AwaitingApproval | PairingState::DisplayingPin)
    }

    /// Check if this is an initiator displaying PIN (no user action needed)
    pub fn is_displaying_pin(&self) -> bool {
        matches!(self.state, PairingState::DisplayingPin)
    }

    /// Check if this is a target waiting for approval
    pub fn is_awaiting_approval(&self) -> bool {
        matches!(self.state, PairingState::AwaitingApproval)
    }

    /// Check if session can be confirmed (only targets can confirm)
    pub fn can_confirm(&self) -> bool {
        matches!(self.state, PairingState::AwaitingApproval) && !self.is_expired()
    }

    /// Mark session as confirmed by target user
    pub fn confirm(&mut self) -> Result<(), String> {
        if !self.can_confirm() {
            return Err(format!("Cannot confirm session in current state: {:?}", self.state));
        }

        self.state = PairingState::Confirmed;
        Ok(())
    }

    /// Update initiator state to displaying PIN after receiving challenge response
    pub fn start_displaying_pin(&mut self) -> Result<(), String> {
        if self.state != PairingState::Initiated {
            return Err(format!("Cannot start displaying PIN from state: {:?}", self.state));
        }

        self.state = PairingState::DisplayingPin;
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
            PairingState::Initiated => "Sending pairing request...".to_string(),
            PairingState::DisplayingPin => format!("Show this PIN to the other device: {}", self.pin),
            PairingState::AwaitingApproval => format!("Accept pairing from {}? PIN: {}", self.peer_name, self.pin),
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

        assert_eq!(session.state, PairingState::AwaitingApproval);
        assert!(session.can_confirm());

        session.confirm().unwrap();
        assert_eq!(session.state, PairingState::Confirmed);
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