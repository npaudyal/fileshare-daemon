// src/network/mod.rs - SIMPLIFIED
pub mod discovery;
pub mod peer;
pub mod protocol;

// Remove all the complex streaming modules
pub use discovery::DiscoveryService;
pub use peer::PeerManager;
pub use protocol::{Message, MessageType, SimpleFileMetadata};
