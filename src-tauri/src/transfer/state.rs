use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TransferState {
    Pending,
    Connecting,
    Negotiating,
    Active,
    Paused,
    Stalled,
    Resuming,
    Finalizing,
    Completed,
    Failed(String),
    Cancelled,
}

impl fmt::Display for TransferState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TransferState::Pending => write!(f, "Pending"),
            TransferState::Connecting => write!(f, "Connecting"),
            TransferState::Negotiating => write!(f, "Negotiating"),
            TransferState::Active => write!(f, "Active"),
            TransferState::Paused => write!(f, "Paused"),
            TransferState::Stalled => write!(f, "Stalled"),
            TransferState::Resuming => write!(f, "Resuming"),
            TransferState::Finalizing => write!(f, "Finalizing"),
            TransferState::Completed => write!(f, "Completed"),
            TransferState::Failed(msg) => write!(f, "Failed: {}", msg),
            TransferState::Cancelled => write!(f, "Cancelled"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TransferDirection {
    Upload,
    Download,
}

impl fmt::Display for TransferDirection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TransferDirection::Upload => write!(f, "Upload"),
            TransferDirection::Download => write!(f, "Download"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TransferError {
    NetworkError(String),
    FileSystemError(String),
    PeerDisconnected,
    ChecksumMismatch,
    InsufficientSpace,
    PermissionDenied,
    Timeout,
    Unknown(String),
}

impl fmt::Display for TransferError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TransferError::NetworkError(msg) => write!(f, "Network error: {}", msg),
            TransferError::FileSystemError(msg) => write!(f, "File system error: {}", msg),
            TransferError::PeerDisconnected => write!(f, "Peer disconnected"),
            TransferError::ChecksumMismatch => write!(f, "File verification failed"),
            TransferError::InsufficientSpace => write!(f, "Insufficient disk space"),
            TransferError::PermissionDenied => write!(f, "Permission denied"),
            TransferError::Timeout => write!(f, "Transfer timed out"),
            TransferError::Unknown(msg) => write!(f, "Unknown error: {}", msg),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum TransferPriority {
    Low = 0,
    Normal = 1,
    High = 2,
    Critical = 3,
}

impl Default for TransferPriority {
    fn default() -> Self {
        TransferPriority::Normal
    }
}

/// State machine transitions for transfer states
impl TransferState {
    pub fn can_transition_to(&self, next: &TransferState) -> bool {
        match (self, next) {
            // From Pending
            (TransferState::Pending, TransferState::Connecting) => true,
            (TransferState::Pending, TransferState::Cancelled) => true,

            // From Connecting
            (TransferState::Connecting, TransferState::Negotiating) => true,
            (TransferState::Connecting, TransferState::Failed(_)) => true,
            (TransferState::Connecting, TransferState::Cancelled) => true,
            (TransferState::Connecting, TransferState::Stalled) => true,

            // From Negotiating
            (TransferState::Negotiating, TransferState::Active) => true,
            (TransferState::Negotiating, TransferState::Failed(_)) => true,
            (TransferState::Negotiating, TransferState::Cancelled) => true,

            // From Active
            (TransferState::Active, TransferState::Paused) => true,
            (TransferState::Active, TransferState::Stalled) => true,
            (TransferState::Active, TransferState::Finalizing) => true,
            (TransferState::Active, TransferState::Failed(_)) => true,
            (TransferState::Active, TransferState::Cancelled) => true,

            // From Paused
            (TransferState::Paused, TransferState::Resuming) => true,
            (TransferState::Paused, TransferState::Cancelled) => true,

            // From Stalled
            (TransferState::Stalled, TransferState::Resuming) => true,
            (TransferState::Stalled, TransferState::Active) => true,
            (TransferState::Stalled, TransferState::Failed(_)) => true,
            (TransferState::Stalled, TransferState::Cancelled) => true,

            // From Resuming
            (TransferState::Resuming, TransferState::Active) => true,
            (TransferState::Resuming, TransferState::Stalled) => true,
            (TransferState::Resuming, TransferState::Failed(_)) => true,
            (TransferState::Resuming, TransferState::Cancelled) => true,

            // From Finalizing
            (TransferState::Finalizing, TransferState::Completed) => true,
            (TransferState::Finalizing, TransferState::Failed(_)) => true,

            // No transitions from terminal states
            (TransferState::Completed, _) => false,
            (TransferState::Failed(_), _) => false,
            (TransferState::Cancelled, _) => false,

            _ => false,
        }
    }
}