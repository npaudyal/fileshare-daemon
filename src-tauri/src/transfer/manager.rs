use super::{Transfer, TransferState, TransferDirection, TransferError, TransferPriority, TransferMetrics};
use crate::{FileshareError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use tokio::sync::{RwLock, mpsc};
use tracing::{info, error, debug};
use uuid::Uuid;

const MAX_CONCURRENT_TRANSFERS: usize = 5;
const CLEANUP_INTERVAL: Duration = Duration::from_secs(300); // 5 minutes
const EVENT_THROTTLE_INTERVAL: Duration = Duration::from_millis(100); // 10Hz max

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferInfo {
    pub id: Uuid,
    pub file_name: String,
    pub file_size: u64,
    pub bytes_transferred: u64,
    pub progress: f32,
    pub speed: u64,
    pub eta: Option<Duration>,
    pub state: TransferState,
    pub direction: TransferDirection,
    pub peer_name: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferDetails {
    pub info: TransferInfo,
    pub peer_id: Uuid,
    #[serde(skip, default = "Instant::now")]
    pub started_at: Instant,
    #[serde(skip)]
    pub completed_at: Option<Instant>,
    pub retry_count: u8,
    pub target_path: PathBuf,
    pub checksum: Option<String>,
    pub priority: TransferPriority,
}

pub struct TransferManager {
    transfers: Arc<RwLock<HashMap<Uuid, Transfer>>>,
    metrics: Arc<RwLock<TransferMetrics>>,
    app_handle: Option<AppHandle>,
    bandwidth_limit: Arc<RwLock<Option<u64>>>,
    event_tx: mpsc::UnboundedSender<TransferEvent>,
    event_rx: Arc<RwLock<mpsc::UnboundedReceiver<TransferEvent>>>,
}

#[derive(Debug, Clone)]
enum TransferEvent {
    Progress { id: Uuid, update: TransferInfo },
    StateChange { id: Uuid, state: TransferState },
    Completed { id: Uuid },
    Failed { id: Uuid, error: String },
}

impl TransferManager {
    pub fn new(app_handle: Option<AppHandle>) -> Self {
        let (event_tx, event_rx) = mpsc::unbounded_channel();

        let manager = Self {
            transfers: Arc::new(RwLock::new(HashMap::new())),
            metrics: Arc::new(RwLock::new(TransferMetrics::default())),
            app_handle,
            bandwidth_limit: Arc::new(RwLock::new(None)),
            event_tx,
            event_rx: Arc::new(RwLock::new(event_rx)),
        };

        // Start background tasks
        manager.start_cleanup_task();
        manager.start_event_processor();

        manager
    }

    /// Add a new transfer to the manager
    pub async fn add_transfer(&self, transfer: Transfer) -> Result<Uuid> {
        let id = transfer.id;

        // Pre-flight checks
        self.validate_transfer(&transfer).await?;

        let mut transfers = self.transfers.write().await;
        let mut metrics = self.metrics.write().await;

        // Check concurrent transfer limit
        let active_count = transfers.values()
            .filter(|t| t.is_active())
            .count();

        let mut transfer = transfer;
        if active_count >= MAX_CONCURRENT_TRANSFERS {
            // Queue the transfer instead of rejecting
            transfer.state = TransferState::Pending;
            transfer.auto_start = false;
        }

        let transfer_file_name = transfer.file_name.clone();
        let transfer_state = transfer.state.clone();

        transfers.insert(id, transfer);
        metrics.transfer_started();

        // Emit event
        let _ = self.event_tx.send(TransferEvent::StateChange {
            id,
            state: transfer_state,
        });

        info!("Added transfer {} for file {}", id, transfer_file_name);

        Ok(id)
    }

    /// Update transfer progress
    pub async fn update_progress(&self, id: Uuid, bytes: u64, speed: u64) -> Result<()> {
        let mut transfers = self.transfers.write().await;

        if let Some(transfer) = transfers.get_mut(&id) {
            transfer.update_progress(bytes, speed);

            // Create info for event
            let info = self.transfer_to_info(transfer);

            // Send throttled event
            let _ = self.event_tx.send(TransferEvent::Progress {
                id,
                update: info,
            });

            // Update metrics
            let mut metrics = self.metrics.write().await;
            let total_bandwidth: u64 = transfers.values()
                .filter(|t| t.is_active())
                .map(|t| t.current_speed)
                .sum();
            metrics.update_bandwidth(total_bandwidth);
        }

        Ok(())
    }

    /// Change transfer state
    pub async fn set_state(&self, id: Uuid, new_state: TransferState) -> Result<()> {
        let mut transfers = self.transfers.write().await;

        if let Some(transfer) = transfers.get_mut(&id) {
            // Validate state transition
            if !transfer.state.can_transition_to(&new_state) {
                return Err(FileshareError::Transfer(
                    format!("Invalid state transition from {} to {}", transfer.state, new_state)
                ));
            }

            let old_state = transfer.state.clone();
            transfer.state = new_state.clone();

            // Handle state-specific logic
            match &new_state {
                TransferState::Active => {
                    debug!("Transfer {} is now active", id);
                }
                TransferState::Completed => {
                    transfer.completed_at = Some(Instant::now());
                    let mut metrics = self.metrics.write().await;
                    metrics.transfer_completed(transfer.bytes_transferred);
                    info!("Transfer {} completed successfully", id);

                    let _ = self.event_tx.send(TransferEvent::Completed { id });
                }
                TransferState::Failed(error) => {
                    transfer.error = Some(TransferError::Unknown(error.clone()));
                    let mut metrics = self.metrics.write().await;
                    metrics.transfer_failed();
                    error!("Transfer {} failed: {}", id, error);

                    let _ = self.event_tx.send(TransferEvent::Failed {
                        id,
                        error: error.clone(),
                    });
                }
                TransferState::Cancelled => {
                    info!("Transfer {} was cancelled", id);
                    // Clean up temp files
                    if transfer.temp_path.exists() {
                        let _ = tokio::fs::remove_file(&transfer.temp_path).await;
                    }
                }
                _ => {}
            }

            // Send state change event
            let _ = self.event_tx.send(TransferEvent::StateChange {
                id,
                state: new_state,
            });

            // Auto-start next pending transfer if one completed
            if matches!(old_state, TransferState::Active) {
                drop(transfers);
                Box::pin(self.start_next_pending()).await;
            }
        }

        Ok(())
    }

    /// Pause a transfer
    pub async fn pause_transfer(&self, id: Uuid) -> Result<()> {
        self.set_state(id, TransferState::Paused).await
    }

    /// Resume a paused transfer
    pub async fn resume_transfer(&self, id: Uuid) -> Result<()> {
        self.set_state(id, TransferState::Resuming).await
    }

    /// Cancel a transfer
    pub async fn cancel_transfer(&self, id: Uuid) -> Result<()> {
        self.set_state(id, TransferState::Cancelled).await?;

        // Remove from active transfers
        let mut transfers = self.transfers.write().await;
        if let Some(transfer) = transfers.remove(&id) {
            // Clean up temp file
            if transfer.temp_path.exists() {
                let _ = tokio::fs::remove_file(&transfer.temp_path).await;
            }
        }

        Ok(())
    }

    /// Get all active transfers
    pub async fn get_active_transfers(&self) -> Vec<TransferInfo> {
        let transfers = self.transfers.read().await;
        transfers.values()
            .filter(|t| !t.is_completed() && !t.is_failed())
            .map(|t| self.transfer_to_info(t))
            .collect()
    }

    /// Get all transfers
    pub async fn get_all_transfers(&self) -> Vec<TransferInfo> {
        let transfers = self.transfers.read().await;
        transfers.values()
            .map(|t| self.transfer_to_info(t))
            .collect()
    }

    /// Get transfer details
    pub async fn get_transfer_details(&self, id: Uuid) -> Option<TransferDetails> {
        let transfers = self.transfers.read().await;
        transfers.get(&id).map(|t| TransferDetails {
            info: self.transfer_to_info(t),
            peer_id: t.peer_id,
            started_at: t.started_at,
            completed_at: t.completed_at,
            retry_count: t.retry_count,
            target_path: t.target_path.clone(),
            checksum: t.checksum.clone(),
            priority: t.priority,
        })
    }

    /// Set bandwidth limit
    pub async fn set_bandwidth_limit(&self, limit: Option<u64>) {
        *self.bandwidth_limit.write().await = limit;
        info!("Bandwidth limit set to: {:?} bytes/sec", limit);
    }

    /// Get current metrics
    pub async fn get_metrics(&self) -> TransferMetrics {
        self.metrics.read().await.clone()
    }

    /// Set transfer priority
    pub async fn set_priority(&self, id: Uuid, priority: TransferPriority) -> Result<()> {
        let mut transfers = self.transfers.write().await;
        if let Some(transfer) = transfers.get_mut(&id) {
            transfer.priority = priority;
            debug!("Transfer {} priority set to {:?}", id, priority);
        }
        Ok(())
    }

    // Private helper methods

    fn transfer_to_info(&self, transfer: &Transfer) -> TransferInfo {
        TransferInfo {
            id: transfer.id,
            file_name: transfer.file_name.clone(),
            file_size: transfer.file_size,
            bytes_transferred: transfer.bytes_transferred,
            progress: transfer.progress_percentage(),
            speed: transfer.current_speed,
            eta: transfer.eta,
            state: transfer.state.clone(),
            direction: transfer.direction.clone(),
            peer_name: transfer.peer_name.clone(),
            error: transfer.error.as_ref().map(|e| e.to_string()),
        }
    }

    async fn validate_transfer(&self, transfer: &Transfer) -> Result<()> {
        // Get parent directory first
        let parent = transfer.target_path.parent()
            .ok_or_else(|| FileshareError::FileOperation("Invalid target path".to_string()))?;

        // Ensure parent directory exists
        if !parent.exists() {
            tokio::fs::create_dir_all(parent).await
                .map_err(|e| FileshareError::FileOperation(format!("Failed to create directory: {}", e)))?;
        }

        // Check disk space on parent directory
        let available = fs2::available_space(parent)
            .map_err(|e| FileshareError::FileOperation(format!("Failed to check disk space: {}", e)))?;

        if available < transfer.file_size {
            return Err(FileshareError::Transfer(format!(
                "Insufficient disk space: need {} bytes, available {} bytes",
                transfer.file_size, available
            )));
        }

        // Test write permission
        let test_file = parent.join(format!(".transfer_test_{}", Uuid::new_v4()));
        tokio::fs::write(&test_file, b"test").await
            .map_err(|e| FileshareError::FileOperation(format!("No write permission: {}", e)))?;
        let _ = tokio::fs::remove_file(&test_file).await;

        Ok(())
    }

    async fn start_next_pending(&self) {
        let mut transfers = self.transfers.write().await;

        // Count active transfers
        let active_count = transfers.values()
            .filter(|t| t.is_active())
            .count();

        if active_count < MAX_CONCURRENT_TRANSFERS {
            // Find highest priority pending transfer
            if let Some((id, _)) = transfers.iter_mut()
                .filter(|(_, t)| matches!(t.state, TransferState::Pending) && t.auto_start)
                .max_by_key(|(_, t)| t.priority)
            {
                let id = *id;
                drop(transfers);
                let _ = Box::pin(self.set_state(id, TransferState::Connecting)).await;
                info!("Auto-started pending transfer {}", id);
            }
        }
    }

    fn start_cleanup_task(&self) {
        let transfers = self.transfers.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(CLEANUP_INTERVAL);

            loop {
                interval.tick().await;

                let mut transfers = transfers.write().await;
                let now = Instant::now();

                // Remove completed transfers older than 5 minutes
                transfers.retain(|id, transfer| {
                    if let Some(completed_at) = transfer.completed_at {
                        if now.duration_since(completed_at) > CLEANUP_INTERVAL {
                            info!("Cleaning up old transfer {}", id);
                            return false;
                        }
                    }
                    true
                });
            }
        });
    }

    fn start_event_processor(&self) {
        let app_handle = self.app_handle.clone();
        let event_rx = self.event_rx.clone();

        tokio::spawn(async move {
            let mut event_rx = event_rx.write().await;
            let mut last_progress_emit = HashMap::new();

            while let Some(event) = event_rx.recv().await {
                match event {
                    TransferEvent::Progress { id, update } => {
                        // Throttle progress events per transfer
                        let now = Instant::now();
                        let should_emit = last_progress_emit.get(&id)
                            .map(|last: &Instant| now.duration_since(*last) >= EVENT_THROTTLE_INTERVAL)
                            .unwrap_or(true);

                        if should_emit {
                            if let Some(app) = &app_handle {
                                let _ = app.emit(
                                    &format!("transfer:progress:{}", id),
                                    &update
                                );
                            }
                            last_progress_emit.insert(id, now);
                        }
                    }
                    TransferEvent::StateChange { id, state } => {
                        if let Some(app) = &app_handle {
                            let _ = app.emit(
                                &format!("transfer:state:{}", id),
                                &state
                            );
                        }
                    }
                    TransferEvent::Completed { id } => {
                        if let Some(app) = &app_handle {
                            let _ = app.emit("transfer:completed", &id);
                        }
                        last_progress_emit.remove(&id);
                    }
                    TransferEvent::Failed { id, error } => {
                        if let Some(app) = &app_handle {
                            let _ = app.emit("transfer:failed", serde_json::json!({
                                "id": id,
                                "error": error
                            }));
                        }
                        last_progress_emit.remove(&id);
                    }
                }
            }
        });
    }
}