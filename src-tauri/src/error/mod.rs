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
}

// Implement From for mdns::Error
impl From<mdns::Error> for FileshareError {
    fn from(err: mdns::Error) -> Self {
        FileshareError::Mdns(err.to_string())
    }
}

// Implement From for rcgen::Error
impl From<rcgen::Error> for FileshareError {
    fn from(err: rcgen::Error) -> Self {
        FileshareError::Config(format!("Certificate generation error: {}", err))
    }
}

// Implement From for rustls::Error
impl From<rustls::Error> for FileshareError {
    fn from(err: rustls::Error) -> Self {
        FileshareError::Network(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("TLS error: {}", err),
        ))
    }
}

// In src/error/mod.rs, add these implementations:

impl From<quinn::WriteError> for FileshareError {
    fn from(e: quinn::WriteError) -> Self {
        FileshareError::Transfer(format!("Write error: {}", e))
    }
}

impl From<quinn::ClosedStream> for FileshareError {
    fn from(_: quinn::ClosedStream) -> Self {
        FileshareError::Transfer("Stream closed".to_string())
    }
}

impl From<quinn::ReadExactError> for FileshareError {
    fn from(e: quinn::ReadExactError) -> Self {
        FileshareError::Transfer(format!("Read error: {}", e))
    }
}

impl From<quinn::ReadError> for FileshareError {
    fn from(e: quinn::ReadError) -> Self {
        FileshareError::Transfer(format!("Read error: {}", e))
    }
}

impl From<tokio::task::JoinError> for FileshareError {
    fn from(e: tokio::task::JoinError) -> Self {
        FileshareError::Transfer(format!("Task join error: {}", e))
    }
}

impl<T> From<tokio::sync::mpsc::error::SendError<T>> for FileshareError {
    fn from(e: tokio::sync::mpsc::error::SendError<T>) -> Self {
        FileshareError::Transfer(format!("Channel send error: {}", e))
    }
}

impl From<std::string::FromUtf8Error> for FileshareError {
    fn from(e: std::string::FromUtf8Error) -> Self {
        FileshareError::Transfer(format!("UTF-8 error: {}", e))
    }
}

impl From<std::num::ParseIntError> for FileshareError {
    fn from(e: std::num::ParseIntError) -> Self {
        FileshareError::Transfer(format!("Parse error: {}", e))
    }
}
