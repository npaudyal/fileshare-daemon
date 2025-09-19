use crate::transfer::progress::SpeedCalculator;
use crate::transfer::types::*;
use dashmap::DashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;
use tauri::{Emitter, Manager};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

pub struct TransferManager {
    transfers: Arc<DashMap<Uuid, Transfer>>,
    speed_calculators: Arc<DashMap<Uuid, RwLock<SpeedCalculator>>>,
    app_handle: Option<tauri::AppHandle>,
    window_mode: Arc<RwLock<WindowMode>>,
}

impl TransferManager {
    pub fn new() -> Self {
        Self {
            transfers: Arc::new(DashMap::new()),
            speed_calculators: Arc::new(DashMap::new()),
            app_handle: None,
            window_mode: Arc::new(RwLock::new(WindowMode::Normal)),
        }
    }

    pub fn set_app_handle(&mut self, handle: tauri::AppHandle) {
        self.app_handle = Some(handle);
    }

    pub async fn create_transfer(
        &self,
        filename: String,
        file_path: PathBuf,
        source_device_id: Uuid,
        source_device_name: String,
        target_path: PathBuf,
        file_size: u64,
        direction: TransferDirection,
    ) -> Uuid {
        let transfer = Transfer::new(
            filename.clone(),
            file_path,
            source_device_id,
            source_device_name.clone(),
            target_path,
            file_size,
            direction.clone(),
        );

        let transfer_id = transfer.id;
        self.transfers.insert(transfer_id, transfer.clone());

        self.speed_calculators
            .insert(transfer_id, RwLock::new(SpeedCalculator::new()));

        self.emit_event("transfer-started", TransferStartedEvent {
            transfer_id,
            filename,
            file_size,
            source_device_name,
            direction,
        });

        self.switch_to_transfer_mode().await;

        info!("✅ Created transfer {}", transfer_id);
        transfer_id
    }

    pub async fn update_progress(&self, transfer_id: Uuid, transferred_bytes: u64) {
        if let Some(mut transfer) = self.transfers.get_mut(&transfer_id) {
            transfer.transferred_bytes = transferred_bytes;
            transfer.status = TransferStatus::InProgress;

            if let Some(calculator_guard) = self.speed_calculators.get(&transfer_id) {
                let mut calculator = calculator_guard.write().await;
                calculator.add_sample(transferred_bytes);

                let speed_bps = calculator.calculate_speed_bps();
                let remaining_bytes = transfer.file_size.saturating_sub(transferred_bytes);
                let eta_seconds = calculator.calculate_eta(remaining_bytes);

                transfer.speed_bps = speed_bps;
                transfer.eta_seconds = eta_seconds;
            }

            let progress_event = transfer.to_progress_event();
            drop(transfer);

            self.emit_event("transfer-progress", progress_event);

            debug!(
                "Transfer {} progress: {:.1}%",
                transfer_id,
                (transferred_bytes as f64 / self.get_transfer(&transfer_id).map_or(1, |t| t.file_size) as f64) * 100.0
            );
        }
    }

    pub fn mark_transfer_completed(&self, transfer_id: Uuid) {
        if let Some(mut transfer) = self.transfers.get_mut(&transfer_id) {
            transfer.status = TransferStatus::Completed;
            transfer.completed_at = Some(SystemTime::now());
            transfer.transferred_bytes = transfer.file_size;

            let filename = transfer.filename.clone();
            drop(transfer);

            self.emit_event("transfer-completed", TransferCompletedEvent {
                transfer_id,
                filename: filename.clone(),
                success: true,
                error: None,
            });

            info!("✅ Transfer {} completed: {}", transfer_id, filename);

            self.check_and_switch_to_normal_mode();
        }
    }

    pub fn mark_transfer_failed(&self, transfer_id: Uuid, error: String) {
        if let Some(mut transfer) = self.transfers.get_mut(&transfer_id) {
            transfer.status = TransferStatus::Failed;
            transfer.completed_at = Some(SystemTime::now());
            transfer.error = Some(error.clone());

            let filename = transfer.filename.clone();
            drop(transfer);

            self.emit_event("transfer-completed", TransferCompletedEvent {
                transfer_id,
                filename: filename.clone(),
                success: false,
                error: Some(error.clone()),
            });

            error!("❌ Transfer {} failed: {} - {}", transfer_id, filename, error);

            self.check_and_switch_to_normal_mode();
        }
    }

    pub async fn pause_transfer(&self, transfer_id: Uuid) -> Result<(), String> {
        if let Some(mut transfer) = self.transfers.get_mut(&transfer_id) {
            if transfer.status == TransferStatus::InProgress {
                transfer.status = TransferStatus::Paused;
                transfer.is_paused = true;
                info!("⏸️ Transfer {} paused", transfer_id);
                Ok(())
            } else {
                Err("Transfer is not in progress".to_string())
            }
        } else {
            Err("Transfer not found".to_string())
        }
    }

    pub async fn resume_transfer(&self, transfer_id: Uuid) -> Result<(), String> {
        if let Some(mut transfer) = self.transfers.get_mut(&transfer_id) {
            if transfer.status == TransferStatus::Paused {
                transfer.status = TransferStatus::InProgress;
                transfer.is_paused = false;
                info!("▶️ Transfer {} resumed", transfer_id);
                Ok(())
            } else {
                Err("Transfer is not paused".to_string())
            }
        } else {
            Err("Transfer not found".to_string())
        }
    }

    pub async fn cancel_transfer(&self, transfer_id: Uuid) -> Result<(), String> {
        if let Some(mut transfer) = self.transfers.get_mut(&transfer_id) {
            transfer.status = TransferStatus::Cancelled;
            transfer.completed_at = Some(SystemTime::now());

            let filename = transfer.filename.clone();
            drop(transfer);

            self.emit_event("transfer-completed", TransferCompletedEvent {
                transfer_id,
                filename: filename.clone(),
                success: false,
                error: Some("Cancelled by user".to_string()),
            });

            info!("🚫 Transfer {} cancelled: {}", transfer_id, filename);

            self.check_and_switch_to_normal_mode();

            self.transfers.remove(&transfer_id);
            self.speed_calculators.remove(&transfer_id);

            Ok(())
        } else {
            Err("Transfer not found".to_string())
        }
    }

    pub fn get_active_transfers(&self) -> Vec<Transfer> {
        self.transfers
            .iter()
            .filter(|entry| {
                matches!(
                    entry.status,
                    TransferStatus::Pending
                        | TransferStatus::Connecting
                        | TransferStatus::InProgress
                        | TransferStatus::Paused
                )
            })
            .map(|entry| entry.value().clone())
            .collect()
    }

    pub fn get_all_transfers(&self) -> Vec<Transfer> {
        self.transfers.iter().map(|entry| entry.value().clone()).collect()
    }

    pub fn get_transfer(&self, transfer_id: &Uuid) -> Option<Transfer> {
        self.transfers.get(transfer_id).map(|entry| entry.value().clone())
    }

    pub async fn set_window_mode(&self, mode: WindowMode) {
        let mut current_mode = self.window_mode.write().await;
        *current_mode = mode.clone();

        self.emit_event("window-mode-changed", mode);

        info!("🪟 Window mode changed to {:?}", current_mode);
    }

    pub async fn get_window_mode(&self) -> WindowMode {
        let mode = self.window_mode.read().await;
        mode.clone()
    }

    async fn switch_to_transfer_mode(&self) {
        let current_mode = self.window_mode.read().await;
        if matches!(*current_mode, WindowMode::Normal) {
            drop(current_mode);
            self.set_window_mode(WindowMode::Transfer).await;

            // Bring window to front when switching to transfer mode
            if let Some(app_handle) = &self.app_handle {
                if let Some(window) = app_handle.get_webview_window("main") {
                    // Show window if hidden
                    if !window.is_visible().unwrap_or(false) {
                        let _ = window.show();
                    }
                    // Focus and bring to front
                    let _ = window.set_focus();
                    let _ = window.set_always_on_top(true);

                    // Remove always on top after a brief moment (keeps it on top initially)
                    let window_clone = window.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                        let _ = window_clone.set_always_on_top(false);
                    });
                }
            }
        }
    }

    fn check_and_switch_to_normal_mode(&self) {
        let active_transfers = self.get_active_transfers();
        if active_transfers.is_empty() {
            let manager = self.clone();
            tokio::spawn(async move {
                tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

                let active_transfers = manager.get_active_transfers();
                if active_transfers.is_empty() {
                    manager.set_window_mode(WindowMode::Normal).await;
                }
            });
        }
    }

    fn emit_event<T: serde::Serialize + Clone>(&self, event: &str, payload: T) {
        if let Some(app_handle) = &self.app_handle {
            if let Err(e) = app_handle.emit(event, payload) {
                warn!("Failed to emit event {}: {}", event, e);
            }
        }
    }

    pub async fn create_mock_transfer(&self) -> Uuid {
        let transfer_id = self.create_transfer(
            "mock_file.zip".to_string(),
            PathBuf::from("/tmp/mock_file.zip"),
            Uuid::new_v4(),
            "Test Device".to_string(),
            PathBuf::from("/downloads/mock_file.zip"),
            100 * 1024 * 1024,
            TransferDirection::Incoming,
        ).await;

        let manager = self.clone();
        tokio::spawn(async move {
            let total_bytes = 100 * 1024 * 1024;
            let chunk_size = total_bytes / 100;

            for i in 0..=100 {
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                let bytes = (i * chunk_size).min(total_bytes);
                manager.update_progress(transfer_id, bytes).await;

                if i == 100 {
                    manager.mark_transfer_completed(transfer_id);
                }
            }
        });

        transfer_id
    }
}

impl Clone for TransferManager {
    fn clone(&self) -> Self {
        Self {
            transfers: self.transfers.clone(),
            speed_calculators: self.speed_calculators.clone(),
            app_handle: self.app_handle.clone(),
            window_mode: self.window_mode.clone(),
        }
    }
}