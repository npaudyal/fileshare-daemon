use crate::{FileshareError, Result};
use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyEvent, GlobalHotKeyManager,
};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

#[derive(Debug, Clone)]
pub enum HotkeyEvent {
    CopyFiles,  // Cmd+Shift+Y (macOS) or Ctrl+Shift+Y (Windows/Linux)
    PasteFiles, // Cmd+Shift+I (macOS) or Ctrl+Shift+I (Windows/Linux)
}

pub struct HotkeyManager {
    manager: GlobalHotKeyManager, // FIXED: Remove Option wrapper
    copy_hotkey: HotKey,
    paste_hotkey: HotKey,
    event_tx: mpsc::UnboundedSender<HotkeyEvent>,
    event_rx: mpsc::UnboundedReceiver<HotkeyEvent>,
}

impl HotkeyManager {
    pub fn new() -> Result<Self> {
        let manager = GlobalHotKeyManager::new().map_err(|e| {
            error!("❌ Failed to create hotkey manager: {}", e);
            FileshareError::Unknown(format!("Failed to create hotkey manager: {}", e))
        })?;

        // FIXED: Use SAME key combinations as working terminal version
        let copy_modifiers = if cfg!(target_os = "macos") {
            Modifiers::META | Modifiers::SHIFT // Cmd+Shift on macOS
        } else {
            Modifiers::CONTROL | Modifiers::SHIFT // Ctrl+Shift on Windows/Linux
        };

        let copy_hotkey = HotKey::new(Some(copy_modifiers), Code::KeyY);
        let paste_hotkey = HotKey::new(Some(copy_modifiers), Code::KeyI);

        let (event_tx, event_rx) = mpsc::unbounded_channel();

        // Log the hotkey combinations
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

        info!("🎹 Hotkey combinations for this platform:");
        info!("  📋 Copy: {}", copy_key_str);
        info!("  📁 Paste: {}", paste_key_str);

        Ok(Self {
            manager,
            copy_hotkey,
            paste_hotkey,
            event_tx,
            event_rx,
        })
    }

    pub async fn start(&mut self) -> Result<()> {
        info!("🎹 Starting hotkey manager...");

        // FIXED: Simple registration - no complex fallbacks
        self.manager.register(self.copy_hotkey).map_err(|e| {
            error!("❌ Failed to register copy hotkey: {}", e);
            FileshareError::Unknown(format!("Failed to register copy hotkey: {}", e))
        })?;

        self.manager.register(self.paste_hotkey).map_err(|e| {
            error!("❌ Failed to register paste hotkey: {}", e);
            FileshareError::Unknown(format!("Failed to register paste hotkey: {}", e))
        })?;

        info!("✅ Copy hotkey registered successfully");
        info!("✅ Paste hotkey registered successfully");

        // FIXED: Use tokio spawn, not std::thread - but keep it simple
        let event_tx = self.event_tx.clone();
        let copy_hotkey = self.copy_hotkey;
        let paste_hotkey = self.paste_hotkey;

        tokio::spawn(async move {
            Self::listen_for_hotkey_events(event_tx, copy_hotkey, paste_hotkey).await;
        });

        info!("✅ Hotkey system initialized successfully");
        Ok(())
    }

    // FIXED: Use the EXACT same listener from working terminal version
    async fn listen_for_hotkey_events(
        event_tx: mpsc::UnboundedSender<HotkeyEvent>,
        copy_hotkey: HotKey,
        paste_hotkey: HotKey,
    ) {
        info!("🎹 Hotkey listener started");

        let receiver = GlobalHotKeyEvent::receiver();

        loop {
            match receiver.try_recv() {
                Ok(event) => {
                    // Only respond to key press events, not release
                    if event.state == global_hotkey::HotKeyState::Pressed {
                        debug!("🎹 Hotkey pressed event received: {:?}", event);

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
                    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    warn!("🎹 Hotkey event receiver disconnected");
                    break;
                }
            }
        }

        info!("🎹 Hotkey listener stopped");
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

        // Unregister hotkeys
        if let Err(e) = self.manager.unregister(self.copy_hotkey) {
            warn!("❌ Failed to unregister copy hotkey: {}", e);
        } else {
            info!("✅ Copy hotkey unregistered");
        }

        if let Err(e) = self.manager.unregister(self.paste_hotkey) {
            warn!("❌ Failed to unregister paste hotkey: {}", e);
        } else {
            info!("✅ Paste hotkey unregistered");
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
