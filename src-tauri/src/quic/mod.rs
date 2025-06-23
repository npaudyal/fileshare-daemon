pub mod protocol;
pub mod connection;
pub mod transfer;
pub mod stream_manager;
pub mod progress;
pub mod integration;
pub mod demo;

// Re-export main types for easier usage
pub use integration::QuicIntegration;
pub use protocol::{QuicMessage, QuicFileMetadata, TransferCapabilities, CompressionType};
pub use transfer::{TransferStatus, TransferDirection};
pub use connection::ConnectionStats;