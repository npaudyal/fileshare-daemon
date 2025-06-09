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
    CopyFiles,  // Cmd+Shift+Y (macOS) or Ctrl+Alt+Y (Windows) or Ctrl+Shift+Y (Linux)
    PasteFiles, // Cmd+Shift+I (macOS) or Ctrl+Alt+I (Windows) or Ctrl+Shift+I (Linux)
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
            error!("❌ Failed to create hotkey manager: {}", e);
            FileshareError::Unknown(format!("Failed to create hotkey manager: {}", e))
        })?;

        // DIFFERENT KEY COMBINATIONS PER PLATFORM
        let (copy_modifiers, paste_modifiers, copy_key_str, paste_key_str) =
            if cfg!(target_os = "macos") {
                (
                    Modifiers::META | Modifiers::SHIFT, // Cmd+Shift on macOS
                    Modifiers::META | Modifiers::SHIFT,
                    "Cmd+Shift+Y",
                    "Cmd+Shift+I",
                )
            } else if cfg!(target_os = "windows") {
                // Use Ctrl+Alt on Windows to avoid conflicts with built-in shortcuts
                (
                    Modifiers::CONTROL | Modifiers::ALT, // Ctrl+Alt on Windows
                    Modifiers::CONTROL | Modifiers::ALT,
                    "Ctrl+Alt+Y",
                    "Ctrl+Alt+I",
                )
            } else {
                // Linux
                (
                    Modifiers::CONTROL | Modifiers::SHIFT, // Ctrl+Shift on Linux
                    Modifiers::CONTROL | Modifiers::SHIFT,
                    "Ctrl+Shift+Y",
                    "Ctrl+Shift+I",
                )
            };

        let copy_hotkey = HotKey::new(Some(copy_modifiers), Code::KeyY);
        let paste_hotkey = HotKey::new(Some(paste_modifiers), Code::KeyI);

        info!("🎹 Hotkey combinations for this platform:");
        info!("  📋 Copy: {}", copy_key_str);
        info!("  📁 Paste: {}", paste_key_str);

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
        info!("🎹 Starting hotkey manager for Windows...");

        if let Some(manager) = &self.manager {
            // Try to register copy hotkey
            match manager.register(self.copy_hotkey) {
                Ok(()) => {
                    info!("✅ Copy hotkey registered successfully");
                }
                Err(e) => {
                    error!("❌ Failed to register copy hotkey: {}", e);

                    // Try alternative key combination for copy
                    let alt_copy_hotkey = if cfg!(target_os = "windows") {
                        // Try Ctrl+Shift+F1 as fallback
                        HotKey::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::F1)
                    } else {
                        return Err(FileshareError::Unknown(format!(
                            "Failed to register copy hotkey: {}",
                            e
                        )));
                    };

                    match manager.register(alt_copy_hotkey) {
                        Ok(()) => {
                            info!("✅ Copy hotkey registered with fallback: Ctrl+Shift+F1");
                            self.copy_hotkey = alt_copy_hotkey;
                        }
                        Err(e2) => {
                            return Err(FileshareError::Unknown(format!(
                                "Failed to register copy hotkey even with fallback: {}",
                                e2
                            )));
                        }
                    }
                }
            }

            // Try to register paste hotkey
            match manager.register(self.paste_hotkey) {
                Ok(()) => {
                    info!("✅ Paste hotkey registered successfully");
                }
                Err(e) => {
                    error!("❌ Failed to register paste hotkey: {}", e);

                    // Try alternative key combination for paste
                    let alt_paste_hotkey = if cfg!(target_os = "windows") {
                        // Try Ctrl+Shift+F2 as fallback
                        HotKey::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::F2)
                    } else {
                        return Err(FileshareError::Unknown(format!(
                            "Failed to register paste hotkey: {}",
                            e
                        )));
                    };

                    match manager.register(alt_paste_hotkey) {
                        Ok(()) => {
                            info!("✅ Paste hotkey registered with fallback: Ctrl+Shift+F2");
                            self.paste_hotkey = alt_paste_hotkey;
                        }
                        Err(e2) => {
                            return Err(FileshareError::Unknown(format!(
                                "Failed to register paste hotkey even with fallback: {}",
                                e2
                            )));
                        }
                    }
                }
            }

            info!("🎹 Starting Windows hotkey event listener...");

            // Start event listener with Windows-specific handling
            let event_tx = self.event_tx.clone();
            let copy_hotkey = self.copy_hotkey;
            let paste_hotkey = self.paste_hotkey;
            let is_running = self.is_running.clone();

            is_running.store(true, Ordering::SeqCst);

            // Create Windows-specific hotkey thread
            let hotkey_thread = std::thread::spawn(move || {
                Self::windows_hotkey_listener(event_tx, copy_hotkey, paste_hotkey, is_running);
            });

            self._hotkey_thread = Some(hotkey_thread);

            // Give Windows time to set up the message pump
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

            info!("✅ Windows hotkey system initialized");
        } else {
            return Err(FileshareError::Unknown(
                "Hotkey manager not initialized".to_string(),
            ));
        }

        Ok(())
    }

    // Windows-specific hotkey listener with message pump
    fn windows_hotkey_listener(
        event_tx: mpsc::UnboundedSender<HotkeyEvent>,
        copy_hotkey: HotKey,
        paste_hotkey: HotKey,
        is_running: Arc<AtomicBool>,
    ) {
        info!("🎹 Windows hotkey listener thread started");

        let receiver = GlobalHotKeyEvent::receiver();
        let mut consecutive_errors = 0;
        const MAX_CONSECUTIVE_ERRORS: u32 = 10;

        while is_running.load(Ordering::SeqCst) {
            match receiver.try_recv() {
                Ok(event) => {
                    consecutive_errors = 0; // Reset error counter on success

                    // Only respond to key press events
                    if event.state == global_hotkey::HotKeyState::Pressed {
                        info!(
                            "🎹 Windows hotkey event received: ID={}, State={:?}",
                            event.id, event.state
                        );

                        let hotkey_event = if event.id == copy_hotkey.id() {
                            info!("🎹 Copy hotkey detected! (ID: {})", event.id);
                            Some(HotkeyEvent::CopyFiles)
                        } else if event.id == paste_hotkey.id() {
                            info!("🎹 Paste hotkey detected! (ID: {})", event.id);
                            Some(HotkeyEvent::PasteFiles)
                        } else {
                            debug!(
                                "🎹 Unknown hotkey ID: {} (Copy: {}, Paste: {})",
                                event.id,
                                copy_hotkey.id(),
                                paste_hotkey.id()
                            );
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
                        debug!("🎹 Ignoring hotkey release event");
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

            // Windows-specific: pump messages occasionally
            if cfg!(target_os = "windows") {
                // Every 100ms, yield to let Windows process messages
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }

        info!("🎹 Windows hotkey listener thread stopped");
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
        info!("🛑 Stopping Windows hotkey manager");

        // Signal the thread to stop
        self.is_running.store(false, Ordering::SeqCst);

        // Unregister hotkeys
        if let Some(manager) = &self.manager {
            if let Err(e) = manager.unregister(self.copy_hotkey) {
                warn!("❌ Failed to unregister copy hotkey: {}", e);
            } else {
                info!("✅ Copy hotkey unregistered");
            }

            if let Err(e) = manager.unregister(self.paste_hotkey) {
                warn!("❌ Failed to unregister paste hotkey: {}", e);
            } else {
                info!("✅ Paste hotkey unregistered");
            }
        }

        // Wait for thread to finish
        if let Some(thread) = self._hotkey_thread.take() {
            std::thread::spawn(move || {
                let _ = thread.join();
            });
        }

        info!("✅ Windows hotkey manager stopped");
        Ok(())
    }
}

impl Drop for HotkeyManager {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
