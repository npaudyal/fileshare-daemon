use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::SystemTime;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transfer {
    pub id: Uuid,
    pub filename: String,
    pub file_path: PathBuf,
    pub source_device_id: Uuid,
    pub source_device_name: String,
    pub target_path: PathBuf,
    pub file_size: u64,
    pub transferred_bytes: u64,
    pub speed_bps: f64,
    pub eta_seconds: Option<u32>,
    pub status: TransferStatus,
    pub direction: TransferDirection,
    pub started_at: SystemTime,
    pub completed_at: Option<SystemTime>,
    pub error: Option<String>,
    pub is_paused: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TransferStatus {
    Pending,
    Connecting,
    InProgress,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TransferDirection {
    Incoming,
    Outgoing,
}

#[derive(Debug, Clone, Serialize)]
pub struct TransferProgressEvent {
    pub transfer_id: Uuid,
    pub progress_percent: f32,
    pub speed_mbps: f64,
    pub eta_seconds: Option<u32>,
    pub transferred_mb: f64,
    pub total_mb: f64,
    pub status: TransferStatus,
}

#[derive(Debug, Clone, Serialize)]
pub struct TransferStartedEvent {
    pub transfer_id: Uuid,
    pub filename: String,
    pub file_size: u64,
    pub source_device_name: String,
    pub direction: TransferDirection,
}

#[derive(Debug, Clone, Serialize)]
pub struct TransferCompletedEvent {
    pub transfer_id: Uuid,
    pub filename: String,
    pub success: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WindowMode {
    Normal,
    Transfer,
}

impl Transfer {
    pub fn new(
        filename: String,
        file_path: PathBuf,
        source_device_id: Uuid,
        source_device_name: String,
        target_path: PathBuf,
        file_size: u64,
        direction: TransferDirection,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            filename,
            file_path,
            source_device_id,
            source_device_name,
            target_path,
            file_size,
            transferred_bytes: 0,
            speed_bps: 0.0,
            eta_seconds: None,
            status: TransferStatus::Pending,
            direction,
            started_at: SystemTime::now(),
            completed_at: None,
            error: None,
            is_paused: false,
        }
    }

    pub fn progress_percent(&self) -> f32 {
        if self.file_size == 0 {
            return 0.0;
        }
        (self.transferred_bytes as f32 / self.file_size as f32) * 100.0
    }

    pub fn to_progress_event(&self) -> TransferProgressEvent {
        TransferProgressEvent {
            transfer_id: self.id,
            progress_percent: self.progress_percent(),
            speed_mbps: self.speed_bps / (1024.0 * 1024.0 * 8.0),
            eta_seconds: self.eta_seconds,
            transferred_mb: self.transferred_bytes as f64 / (1024.0 * 1024.0),
            total_mb: self.file_size as f64 / (1024.0 * 1024.0),
            status: self.status.clone(),
        }
    }
}