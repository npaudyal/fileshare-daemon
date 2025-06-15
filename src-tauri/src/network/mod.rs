// src/network/mod.rs - COMPLETE UPDATE
pub mod connection_pool;
pub mod discovery;
pub mod peer;
pub mod protocol;
pub mod streaming_protocol;
pub mod streaming_reader;
pub mod streaming_writer;

// NEW: High-speed streaming modules
pub mod high_speed_memory;
pub mod high_speed_pipeline;
pub mod high_speed_streaming;
pub mod performance_monitor;

// Re-export everything
pub use connection_pool::ConnectionPoolManager;
pub use discovery::DiscoveryService;
pub use peer::PeerManager;
pub use protocol::{FileMetadata, Message, MessageType, TransferChunk};
pub use streaming_protocol::{StreamChunkHeader, StreamingConfig, StreamingFileMetadata};
pub use streaming_reader::{StreamingChunkReader, StreamingFileReader};
pub use streaming_writer::StreamingFileWriter;

// NEW: High-speed exports
pub use high_speed_memory::{HighSpeedMemoryPool, MemoryPoolStats};
pub use high_speed_streaming::{HighSpeedConfig, HighSpeedProgress, HighSpeedStreamingManager};
pub use performance_monitor::{PerformanceMetrics, PerformanceMonitor};
