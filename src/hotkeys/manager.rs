use crate::{FileshareError, Result};
use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyEvent, GlobalHotKeyManager,
};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

#[derive(Debug, Clone)]
pub enum HotkeyEvent {
    CopyFiles,  // Cmd+Shift+Y
    PasteFiles, // Cmd+Shift+I
}

pub struct HotkeyManager {
    manager: GlobalHotKeyManager,
    copy_hotkey: HotKey,
    paste_hotkey: HotKey,
    event_tx: mpsc::UnboundedSender<HotkeyEvent>,
    event_rx: mpsc::UnboundedReceiver<HotkeyEvent>,
}

impl HotkeyManager {
    pub fn new() -> Result<Self> {
        let manager = GlobalHotKeyManager::new().map_err(|e| {
            FileshareError::Unknown(format!("Failed to create hotkey manager: {}", e))
        })?;

        // Define hotkeys
        // Cmd+Shift+Y (or Ctrl+Shift+Y on Windows/Linux)
        let copy_modifiers = if cfg!(target_os = "macos") {
            Modifiers::META | Modifiers::SHIFT // Cmd+Shift on macOS
        } else {
            Modifiers::CONTROL | Modifiers::SHIFT // Ctrl+Shift on Windows/Linux
        };

        let copy_hotkey = HotKey::new(Some(copy_modifiers), Code::KeyY);

        // Cmd+Shift+I (or Ctrl+Shift+I on Windows/Linux)
        let paste_hotkey = HotKey::new(Some(copy_modifiers), Code::KeyI);

        let (event_tx, event_rx) = mpsc::unbounded_channel();

        Ok(Self {
            manager,
            copy_hotkey,
            paste_hotkey,
            event_tx,
            event_rx,
        })
    }

    pub async fn start(&mut self) -> Result<()> {
        info!("Starting hotkey manager");

        // Register hotkeys
        self.manager.register(self.copy_hotkey).map_err(|e| {
            FileshareError::Unknown(format!("Failed to register copy hotkey: {}", e))
        })?;

        self.manager.register(self.paste_hotkey).map_err(|e| {
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

        info!("Registered global hotkeys:");
        info!("  Copy files: {}", copy_key_str);
        info!("  Paste files: {}", paste_key_str);

        // Start event listener
        let event_tx = self.event_tx.clone();
        let copy_hotkey = self.copy_hotkey;
        let paste_hotkey = self.paste_hotkey;

        tokio::spawn(async move {
            Self::listen_for_hotkey_events(event_tx, copy_hotkey, paste_hotkey).await;
        });

        Ok(())
    }

    async fn listen_for_hotkey_events(
        event_tx: mpsc::UnboundedSender<HotkeyEvent>,
        copy_hotkey: HotKey,
        paste_hotkey: HotKey,
    ) {
        info!("Listening for hotkey events");

        // Get the receiver from global-hotkey
        let receiver = GlobalHotKeyEvent::receiver();

        loop {
            // Use the crossbeam channel receiver directly
            match receiver.try_recv() {
                Ok(event) => {
                    debug!("Hotkey event received: {:?}", event);

                    let hotkey_event = if event.id == copy_hotkey.id() {
                        Some(HotkeyEvent::CopyFiles)
                    } else if event.id == paste_hotkey.id() {
                        Some(HotkeyEvent::PasteFiles)
                    } else {
                        None
                    };

                    if let Some(hotkey_event) = hotkey_event {
                        info!("Hotkey triggered: {:?}", hotkey_event);
                        if let Err(e) = event_tx.send(hotkey_event) {
                            error!("Failed to send hotkey event: {}", e);
                            break;
                        }
                    }
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    // No events, wait a bit
                    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    warn!("Hotkey event receiver disconnected");
                    break;
                }
            }
        }
    }

    pub async fn get_event(&mut self) -> Option<HotkeyEvent> {
        self.event_rx.recv().await
    }

    pub fn try_get_event(&mut self) -> Option<HotkeyEvent> {
        self.event_rx.try_recv().ok()
    }

    pub fn stop(&mut self) -> Result<()> {
        info!("Stopping hotkey manager");

        self.manager.unregister(self.copy_hotkey).map_err(|e| {
            FileshareError::Unknown(format!("Failed to unregister copy hotkey: {}", e))
        })?;

        self.manager.unregister(self.paste_hotkey).map_err(|e| {
            FileshareError::Unknown(format!("Failed to unregister paste hotkey: {}", e))
        })?;

        info!("Hotkey manager stopped");
        Ok(())
    }
}

impl Drop for HotkeyManager {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
