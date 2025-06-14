use thiserror::Error;

pub type Result<T> = std::result::Result<T, FileshareError>;

#[derive(Error, Debug)]
pub enum FileshareError {
    #[error("Network error: {0}")]
    Network(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] bincode::Error),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Discovery error: {0}")]
    Discovery(String),

    #[error("mDNS error: {0}")]
    Mdns(String),

    #[error("Transfer error: {0}")]
    Transfer(String),

    #[error("System tray error: {0}")]
    Tray(String),

    #[error("File operation error: {0}")]
    FileOperation(String),

    #[error("Authentication error: {0}")]
    Authentication(String),

    #[error("Unknown error: {0}")]
    Unknown(String),
    #[error("Streaming error: {0}")]
    Streaming(String),

    #[error("Connection pool error: {0}")]
    ConnectionPool(String),

    #[error("Hybrid transfer error: {0}")]
    HybridTransfer(String),
}

// Implement From for mdns::Error
impl From<mdns::Error> for FileshareError {
    fn from(err: mdns::Error) -> Self {
        FileshareError::Mdns(err.to_string())
    }
}
