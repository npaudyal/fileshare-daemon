use crate::Result;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tracing::{error, info, warn};
use uuid::Uuid;

// Event callback type for progress updates
pub type ProgressCallback = Arc<dyn Fn(&Transfer) + Send + Sync>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TransferDirection {
    Upload,
    Download,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TransferStatus {
    Preparing,
    Active,
    Paused,
    Completed,
    Error(String),
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transfer {
    pub id: Uuid,
    pub file_name: String,
    pub file_path: PathBuf,
    pub file_size: u64,
    pub transferred_bytes: u64,
    pub progress: f32,
    pub speed_bps: u64,
    pub direction: TransferDirection,
    pub status: TransferStatus,
    pub peer_id: Uuid,
    pub peer_name: String,
    pub start_time: u64,
    pub end_time: Option<u64>,
    pub time_remaining: Option<u64>,
    pub error: Option<String>,
    pub is_paused: bool,
}

impl Transfer {
    pub fn new(
        file_name: String,
        file_path: PathBuf,
        file_size: u64,
        direction: TransferDirection,
        peer_id: Uuid,
        peer_name: String,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            file_name,
            file_path,
            file_size,
            transferred_bytes: 0,
            progress: 0.0,
            speed_bps: 0,
            direction,
            status: TransferStatus::Preparing,
            peer_id,
            peer_name,
            start_time: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
            end_time: None,
            time_remaining: None,
            error: None,
            is_paused: false,
        }
    }

    pub fn update_progress(&mut self, bytes_transferred: u64) {
        self.transferred_bytes = bytes_transferred;
        self.progress = if self.file_size > 0 {
            (bytes_transferred as f32 / self.file_size as f32) * 100.0
        } else {
            0.0
        };

        // Calculate speed based on time elapsed
        let elapsed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
            - self.start_time;

        if elapsed > 0 {
            self.speed_bps = (bytes_transferred * 1000) / elapsed;

            // Calculate time remaining
            if self.speed_bps > 0 && bytes_transferred < self.file_size {
                let remaining_bytes = self.file_size - bytes_transferred;
                self.time_remaining = Some(remaining_bytes / self.speed_bps);
            }
        }

        // Update status if transfer is complete
        if bytes_transferred >= self.file_size {
            self.status = TransferStatus::Completed;
            self.end_time = Some(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64,
            );
        } else if !self.is_paused {
            self.status = TransferStatus::Active;
        }
    }

    pub fn pause(&mut self) {
        if matches!(self.status, TransferStatus::Active) {
            self.status = TransferStatus::Paused;
            self.is_paused = true;
        }
    }

    pub fn resume(&mut self) {
        if matches!(self.status, TransferStatus::Paused) {
            self.status = TransferStatus::Active;
            self.is_paused = false;
        }
    }

    pub fn cancel(&mut self) {
        self.status = TransferStatus::Cancelled;
        self.end_time = Some(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
        );
    }

    pub fn set_error(&mut self, error: String) {
        self.status = TransferStatus::Error(error.clone());
        self.error = Some(error);
        self.end_time = Some(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
        );
    }
}

pub struct TransferManager {
    active_transfers: Arc<DashMap<Uuid, Transfer>>,
    completed_transfers: Arc<RwLock<Vec<Transfer>>>,
    max_completed_history: usize,
    progress_callback: Option<ProgressCallback>,
}

impl TransferManager {
    pub fn new() -> Self {
        Self {
            active_transfers: Arc::new(DashMap::new()),
            completed_transfers: Arc::new(RwLock::new(Vec::new())),
            max_completed_history: 50,
            progress_callback: None,
        }
    }

    pub fn with_progress_callback(callback: ProgressCallback) -> Self {
        Self {
            active_transfers: Arc::new(DashMap::new()),
            completed_transfers: Arc::new(RwLock::new(Vec::new())),
            max_completed_history: 50,
            progress_callback: Some(callback),
        }
    }

    pub fn set_progress_callback(&mut self, callback: ProgressCallback) {
        self.progress_callback = Some(callback);
    }

    pub async fn create_transfer(
        &self,
        file_name: String,
        file_path: PathBuf,
        file_size: u64,
        direction: TransferDirection,
        peer_id: Uuid,
        peer_name: String,
    ) -> Uuid {
        let transfer = Transfer::new(
            file_name,
            file_path,
            file_size,
            direction,
            peer_id,
            peer_name,
        );
        let transfer_id = transfer.id;

        self.active_transfers.insert(transfer_id, transfer);
        info!("Created new transfer: {}", transfer_id);

        transfer_id
    }

    pub async fn update_transfer_progress(
        &self,
        transfer_id: Uuid,
        bytes_transferred: u64,
    ) -> Result<()> {
        if let Some(mut transfer) = self.active_transfers.get_mut(&transfer_id) {
            transfer.update_progress(bytes_transferred);

            // Emit progress event
            if let Some(callback) = &self.progress_callback {
                callback(&transfer);
            }

            // If transfer is completed, move to completed list
            if matches!(transfer.status, TransferStatus::Completed) {
                let completed_transfer = transfer.clone();
                drop(transfer);

                self.active_transfers.remove(&transfer_id);

                let mut completed = self.completed_transfers.write().await;
                completed.push(completed_transfer.clone());

                // Keep only recent completed transfers
                if completed.len() > self.max_completed_history {
                    let excess = completed.len() - self.max_completed_history;
                    completed.drain(0..excess);
                }

                // Emit completion event
                if let Some(callback) = &self.progress_callback {
                    callback(&completed_transfer);
                }
            }
        } else {
            warn!("Transfer {} not found for progress update", transfer_id);
        }

        Ok(())
    }

    pub async fn pause_transfer(&self, transfer_id: Uuid) -> Result<()> {
        if let Some(mut transfer) = self.active_transfers.get_mut(&transfer_id) {
            transfer.pause();

            // Emit pause event
            if let Some(callback) = &self.progress_callback {
                callback(&transfer);
            }

            info!("Paused transfer: {}", transfer_id);
        } else {
            warn!("Transfer {} not found for pause", transfer_id);
        }
        Ok(())
    }

    pub async fn resume_transfer(&self, transfer_id: Uuid) -> Result<()> {
        if let Some(mut transfer) = self.active_transfers.get_mut(&transfer_id) {
            transfer.resume();

            // Emit resume event
            if let Some(callback) = &self.progress_callback {
                callback(&transfer);
            }

            info!("Resumed transfer: {}", transfer_id);
        } else {
            warn!("Transfer {} not found for resume", transfer_id);
        }
        Ok(())
    }

    pub async fn cancel_transfer(&self, transfer_id: Uuid) -> Result<()> {
        if let Some(mut transfer) = self.active_transfers.get_mut(&transfer_id) {
            transfer.cancel();
            let cancelled_transfer = transfer.clone();
            drop(transfer);

            self.active_transfers.remove(&transfer_id);

            let mut completed = self.completed_transfers.write().await;
            completed.push(cancelled_transfer.clone());

            // Emit cancellation event
            if let Some(callback) = &self.progress_callback {
                callback(&cancelled_transfer);
            }

            info!("Cancelled transfer: {}", transfer_id);
        } else {
            warn!("Transfer {} not found for cancellation", transfer_id);
        }
        Ok(())
    }

    pub async fn set_transfer_error(&self, transfer_id: Uuid, error: String) -> Result<()> {
        if let Some(mut transfer) = self.active_transfers.get_mut(&transfer_id) {
            transfer.set_error(error);
            let error_transfer = transfer.clone();
            drop(transfer);

            self.active_transfers.remove(&transfer_id);

            let mut completed = self.completed_transfers.write().await;
            completed.push(error_transfer.clone());

            // Emit error event
            if let Some(callback) = &self.progress_callback {
                callback(&error_transfer);
            }

            error!("Transfer {} failed with error", transfer_id);
        } else {
            warn!("Transfer {} not found for error update", transfer_id);
        }
        Ok(())
    }

    pub async fn get_active_transfers(&self) -> Vec<Transfer> {
        self.active_transfers
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    pub async fn get_completed_transfers(&self) -> Vec<Transfer> {
        self.completed_transfers.read().await.clone()
    }

    pub async fn get_all_transfers(&self) -> Vec<Transfer> {
        let mut transfers = self.get_active_transfers().await;
        transfers.extend(self.get_completed_transfers().await);
        transfers
    }

    pub async fn get_transfer(&self, transfer_id: Uuid) -> Option<Transfer> {
        if let Some(transfer) = self.active_transfers.get(&transfer_id) {
            return Some(transfer.clone());
        }

        let completed = self.completed_transfers.read().await;
        completed.iter().find(|t| t.id == transfer_id).cloned()
    }

    pub async fn toggle_transfer_pause(&self, transfer_id: Uuid) -> Result<()> {
        if let Some(transfer) = self.active_transfers.get(&transfer_id) {
            if transfer.is_paused {
                drop(transfer);
                self.resume_transfer(transfer_id).await
            } else {
                drop(transfer);
                self.pause_transfer(transfer_id).await
            }
        } else {
            warn!("Transfer {} not found for toggle", transfer_id);
            Ok(())
        }
    }

    pub async fn cleanup_old_transfers(&self) {
        let mut completed = self.completed_transfers.write().await;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        // Remove transfers older than 1 hour
        completed.retain(|t| {
            if let Some(end_time) = t.end_time {
                now - end_time < 3600000 // 1 hour in milliseconds
            } else {
                true
            }
        });
    }
}