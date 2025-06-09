use crate::{FileshareError, Result};
use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyEvent, GlobalHotKeyManager,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

#[derive(Debug, Clone)]
pub enum HotkeyEvent {
    CopyFiles,  // Cmd+Shift+Y (macOS) or Ctrl+Shift+Y (Windows/Linux)
    PasteFiles, // Cmd+Shift+I (macOS) or Ctrl+Shift+I (Windows/Linux)
}

pub struct HotkeyManager {
    manager: Option<GlobalHotKeyManager>,
    copy_hotkey: HotKey,
    paste_hotkey: HotKey,
    event_tx: mpsc::UnboundedSender<HotkeyEvent>,
    event_rx: mpsc::UnboundedReceiver<HotkeyEvent>,
    is_running: Arc<AtomicBool>,
    _hotkey_thread: Option<std::thread::JoinHandle<()>>,
}

impl HotkeyManager {
    pub fn new() -> Result<Self> {
        let manager = GlobalHotKeyManager::new().map_err(|e| {
            FileshareError::Unknown(format!("Failed to create hotkey manager: {}", e))
        })?;

        // Define hotkeys based on platform
        let copy_modifiers = if cfg!(target_os = "macos") {
            Modifiers::META | Modifiers::SHIFT // Cmd+Shift on macOS
        } else {
            Modifiers::CONTROL | Modifiers::SHIFT // Ctrl+Shift on Windows/Linux
        };

        let copy_hotkey = HotKey::new(Some(copy_modifiers), Code::KeyY);
        let paste_hotkey = HotKey::new(Some(copy_modifiers), Code::KeyI);

        let (event_tx, event_rx) = mpsc::unbounded_channel();

        Ok(Self {
            manager: Some(manager),
            copy_hotkey,
            paste_hotkey,
            event_tx,
            event_rx,
            is_running: Arc::new(AtomicBool::new(false)),
            _hotkey_thread: None,
        })
    }

    pub async fn start(&mut self) -> Result<()> {
        info!("🎹 Starting hotkey manager");

        if let Some(manager) = &self.manager {
            // Register hotkeys
            manager.register(self.copy_hotkey).map_err(|e| {
                error!("❌ Failed to register copy hotkey: {}", e);
                FileshareError::Unknown(format!("Failed to register copy hotkey: {}", e))
            })?;

            manager.register(self.paste_hotkey).map_err(|e| {
                error!("❌ Failed to register paste hotkey: {}", e);
                FileshareError::Unknown(format!("Failed to register paste hotkey: {}", e))
            })?;

            let copy_key_str = if cfg!(target_os = "macos") {
                "Cmd+Shift+Y"
            } else {
                "Ctrl+Shift+Y"
            };
            let paste_key_str = if cfg!(target_os = "macos") {
                "Cmd+Shift+I"
            } else {
                "Ctrl+Shift+I"
            };

            info!("✅ Registered global hotkeys:");
            info!("  📋 Copy files: {}", copy_key_str);
            info!("  📁 Paste files: {}", paste_key_str);

            // Start event listener in a dedicated thread
            let event_tx = self.event_tx.clone();
            let copy_hotkey = self.copy_hotkey;
            let paste_hotkey = self.paste_hotkey;
            let is_running = self.is_running.clone();

            is_running.store(true, Ordering::SeqCst);

            // Spawn dedicated thread for hotkey events (this is crucial for Windows)
            let hotkey_thread = std::thread::spawn(move || {
                Self::listen_for_hotkey_events_sync(
                    event_tx,
                    copy_hotkey,
                    paste_hotkey,
                    is_running,
                );
            });

            self._hotkey_thread = Some(hotkey_thread);

            // Give the thread a moment to start
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

            info!("🎹 Hotkey event listener thread started");
        } else {
            return Err(FileshareError::Unknown(
                "Hotkey manager not initialized".to_string(),
            ));
        }

        Ok(())
    }

    // CRITICAL: Synchronous event listener for hotkeys (runs in std::thread)
    fn listen_for_hotkey_events_sync(
        event_tx: mpsc::UnboundedSender<HotkeyEvent>,
        copy_hotkey: HotKey,
        paste_hotkey: HotKey,
        is_running: Arc<AtomicBool>,
    ) {
        info!("🎹 Hotkey event listener thread started successfully");

        let receiver = GlobalHotKeyEvent::receiver();

        while is_running.load(Ordering::SeqCst) {
            match receiver.try_recv() {
                Ok(event) => {
                    // Only respond to key press events, not release
                    if event.state == global_hotkey::HotKeyState::Pressed {
                        debug!("🎹 Hotkey pressed event received: {:?}", event);

                        let hotkey_event = if event.id == copy_hotkey.id() {
                            info!("🎹 Copy hotkey detected!");
                            Some(HotkeyEvent::CopyFiles)
                        } else if event.id == paste_hotkey.id() {
                            info!("🎹 Paste hotkey detected!");
                            Some(HotkeyEvent::PasteFiles)
                        } else {
                            debug!("🎹 Unknown hotkey ID: {}", event.id);
                            None
                        };

                        if let Some(hotkey_event) = hotkey_event {
                            info!("🎹 Sending hotkey event: {:?}", hotkey_event);
                            if let Err(e) = event_tx.send(hotkey_event) {
                                error!("❌ Failed to send hotkey event: {}", e);
                                break;
                            }
                        }
                    } else {
                        debug!("🎹 Ignoring hotkey release event: {:?}", event);
                    }
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    // No events, sleep briefly to prevent CPU spinning
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    warn!("🎹 Hotkey event receiver disconnected");
                    break;
                }
            }
        }
        info!("🎹 Hotkey event listener thread stopped");
    }

    pub async fn get_event(&mut self) -> Option<HotkeyEvent> {
        self.event_rx.recv().await
    }

    pub fn try_get_event(&mut self) -> Option<HotkeyEvent> {
        self.event_rx.try_recv().ok()
    }

    pub fn get_event_sender(&self) -> mpsc::UnboundedSender<HotkeyEvent> {
        self.event_tx.clone()
    }

    pub fn stop(&mut self) -> Result<()> {
        info!("🛑 Stopping hotkey manager");

        // Signal the thread to stop
        self.is_running.store(false, Ordering::SeqCst);

        // Unregister hotkeys
        if let Some(manager) = &self.manager {
            if let Err(e) = manager.unregister(self.copy_hotkey) {
                warn!("❌ Failed to unregister copy hotkey: {}", e);
            }

            if let Err(e) = manager.unregister(self.paste_hotkey) {
                warn!("❌ Failed to unregister paste hotkey: {}", e);
            }
        }

        // Wait for thread to finish (with timeout)
        if let Some(thread) = self._hotkey_thread.take() {
            // Give thread 1 second to finish gracefully
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(1));
                if !thread.is_finished() {
                    warn!("🎹 Hotkey thread did not finish gracefully");
                }
            });
        }

        info!("✅ Hotkey manager stopped");
        Ok(())
    }
}

impl Drop for HotkeyManager {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
