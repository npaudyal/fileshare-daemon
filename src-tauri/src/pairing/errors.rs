use thiserror::Error;

#[derive(Error, Debug)]
pub enum PairingError {
    #[error("Pairing session not found")]
    SessionNotFound,
    
    #[error("Pairing session expired")]
    SessionExpired,
    
    #[error("Invalid PIN")]
    InvalidPin,
    
    #[error("Maximum PIN attempts exceeded")]
    MaxAttemptsExceeded,
    
    #[error("Device already paired")]
    AlreadyPaired,
    
    #[error("Device blocked")]
    DeviceBlocked,
    
    #[error("Pairing rejected by remote device")]
    Rejected(String),
    
    #[error("Cryptographic error: {0}")]
    CryptoError(String),
    
    #[error("Network error: {0}")]
    NetworkError(String),
    
    #[error("Storage error: {0}")]
    StorageError(String),
    
    #[error("Invalid state transition")]
    InvalidStateTransition,
    
    #[error("Timeout waiting for response")]
    Timeout,
}

impl From<PairingError> for crate::FileshareError {
    fn from(err: PairingError) -> Self {
        crate::FileshareError::Unknown(format!("Pairing error: {}", err))
    }
}