pub mod clipboard;
pub mod config; // Add this

pub mod error;
pub mod hotkeys; // Add this
pub mod network;
pub mod service;
pub mod tray;
pub mod ui; // Add this
pub mod utils;

pub use error::{FileshareError, Result};
