pub mod clipboard;
pub mod config;
pub mod error;
pub mod hotkeys;
pub mod http;
pub mod network;
pub mod pairing;
pub mod quic;
pub mod service;
pub mod transfer;
pub mod utils;

pub use error::{FileshareError, Result};
