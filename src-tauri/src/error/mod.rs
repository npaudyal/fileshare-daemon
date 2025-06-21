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

    #[error("Streaming error: {0}")]
    Streaming(#[from] StreamingError), // NEW

    #[error("Unknown error: {0}")]
    Unknown(String),
}

#[derive(Error, Debug)]
pub enum StreamingError {
    #[error("Invalid chunk size: {size} bytes")]
    InvalidChunkSize { size: usize },

    #[error("File too large for available memory: {size} bytes")]
    FileTooLargeForMemory { size: u64 },

    #[error("Streaming interrupted at byte {position}")]
    StreamingInterrupted { position: u64 },

    #[error("Chunk validation failed: chunk {index}, expected {expected}, got {actual}")]
    ChunkValidationFailed {
        index: u64,
        expected: u32,
        actual: u32,
    },

    #[error("File changed during transfer: original size {original}, current size {current}")]
    FileChangedDuringTransfer { original: u64, current: u64 },

    #[error("Insufficient disk space: need {needed} bytes, available {available} bytes")]
    InsufficientDiskSpace { needed: u64, available: u64 },
}

// Implement From for mdns::Error
impl From<mdns::Error> for FileshareError {
    fn from(err: mdns::Error) -> Self {
        FileshareError::Mdns(err.to_string())
    }
}
