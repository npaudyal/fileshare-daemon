pub mod blazing_transfer;
pub mod connection;
pub mod optimized_transfer;
pub mod protocol;
pub mod stream_manager;
pub mod transfer;

pub use blazing_transfer::{BlazingReceiver, BlazingTransfer};
pub use connection::{QuicConnection, QuicConnectionManager};
pub use optimized_transfer::{OptimizedReceiver, OptimizedTransfer};
pub use protocol::{QuicMessage, StreamType};
pub use stream_manager::StreamManager;
pub use transfer::{ParallelTransferManager, QuicFileTransfer};

// Re-export key types for convenience
pub use quinn::{TransportConfig, VarInt};
