pub mod discovery;
pub mod peer_quic;
pub mod protocol;

pub use discovery::DiscoveryService;
pub use peer_quic::PeerManager;
pub use protocol::{Message, MessageType};
