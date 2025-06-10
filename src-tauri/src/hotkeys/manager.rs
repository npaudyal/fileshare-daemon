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
    CopyFiles,
    PasteFiles,
}

pub struct HotkeyManager {
    manager: Option<GlobalHotKeyManager>,
    copy_hotkey: Option<HotKey>,
    paste_hotkey: Option<HotKey>,
    event_tx: mpsc::UnboundedSender<HotkeyEvent>,
    event_rx: mpsc::UnboundedReceiver<HotkeyEvent>,
    is_running: Arc<AtomicBool>,
    _hotkey_thread: Option<std::thread::JoinHandle<()>>,
}

impl HotkeyManager {
    pub fn new() -> Result<Self> {
        info!("🎹 Creating hotkey manager...");

        let manager = GlobalHotKeyManager::new().map_err(|e| {
            error!("❌ Failed to create hotkey manager: {}", e);
            FileshareError::Unknown(format!("Failed to create hotkey manager: {}", e))
        })?;

        info!("✅ Hotkey manager created successfully");
        let (event_tx, event_rx) = mpsc::unbounded_channel();

        Ok(Self {
            manager: Some(manager),
            copy_hotkey: None,
            paste_hotkey: None,
            event_tx,
            event_rx,
            is_running: Arc::new(AtomicBool::new(false)),
            _hotkey_thread: None,
        })
    }

    pub async fn start(&mut self) -> Result<()> {
        info!("🎹 Starting hotkey manager...");

        if let Some(manager) = &self.manager {
            // Test basic hotkey registration first
            info!("🧪 Testing basic hotkey registration capabilities...");

            // Try a very uncommon combination first to test if registration works at all
            let test_hotkey = HotKey::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::F12);
            match manager.register(test_hotkey) {
                Ok(()) => {
                    info!("✅ Basic hotkey registration works");
                    let _ = manager.unregister(test_hotkey);
                }
                Err(e) => {
                    error!("❌ Basic hotkey registration failed: {}", e);
                    return Err(FileshareError::Unknown(format!(
                        "Hotkey registration not working: {}",
                        e
                    )));
                }
            }

            // Platform-specific key combinations
            let (copy_modifiers, paste_modifiers, copy_key_str, paste_key_str) =
                if cfg!(target_os = "macos") {
                    (
                        Modifiers::META | Modifiers::SHIFT,
                        Modifiers::META | Modifiers::SHIFT,
                        "Cmd+Shift+Y",
                        "Cmd+Shift+I",
                    )
                } else {
                    // Windows and Linux
                    (
                        Modifiers::CONTROL | Modifiers::SHIFT,
                        Modifiers::CONTROL | Modifiers::SHIFT,
                        "Ctrl+Shift+Y",
                        "Ctrl+Shift+I",
                    )
                };

            let copy_hotkey = HotKey::new(Some(copy_modifiers), Code::KeyY);
            let paste_hotkey = HotKey::new(Some(paste_modifiers), Code::KeyI);

            info!("🎹 Attempting to register hotkey combinations:");
            info!(
                "  📋 Copy: {} (Modifiers: {:?}, Code: {:?})",
                copy_key_str,
                copy_modifiers,
                Code::KeyY
            );
            info!(
                "  📁 Paste: {} (Modifiers: {:?}, Code: {:?})",
                paste_key_str,
                paste_modifiers,
                Code::KeyI
            );

            // Try to register copy hotkey
            match manager.register(copy_hotkey) {
                Ok(()) => {
                    info!("✅ Copy hotkey registered successfully: {}", copy_key_str);
                    self.copy_hotkey = Some(copy_hotkey);
                }
                Err(e) => {
                    error!("❌ Failed to register copy hotkey {}: {}", copy_key_str, e);

                    // Try fallback
                    let fallback_copy =
                        HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyY);
                    match manager.register(fallback_copy) {
                        Ok(()) => {
                            info!("✅ Copy hotkey registered with fallback: Ctrl+Alt+Y");
                            self.copy_hotkey = Some(fallback_copy);
                        }
                        Err(e2) => {
                            error!("❌ Failed to register copy fallback: {}", e2);
                            return Err(FileshareError::Unknown(format!(
                                "Cannot register copy hotkey: {}",
                                e2
                            )));
                        }
                    }
                }
            }

            // Try to register paste hotkey
            match manager.register(paste_hotkey) {
                Ok(()) => {
                    info!("✅ Paste hotkey registered successfully: {}", paste_key_str);
                    self.paste_hotkey = Some(paste_hotkey);
                }
                Err(e) => {
                    error!(
                        "❌ Failed to register paste hotkey {}: {}",
                        paste_key_str, e
                    );

                    // Try fallback
                    let fallback_paste =
                        HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyI);
                    match manager.register(fallback_paste) {
                        Ok(()) => {
                            info!("✅ Paste hotkey registered with fallback: Ctrl+Alt+I");
                            self.paste_hotkey = Some(fallback_paste);
                        }
                        Err(e2) => {
                            error!("❌ Failed to register paste fallback: {}", e2);
                            return Err(FileshareError::Unknown(format!(
                                "Cannot register paste hotkey: {}",
                                e2
                            )));
                        }
                    }
                }
            }

            // Start the event listener
            self.start_event_listener().await?;
        } else {
            return Err(FileshareError::Unknown(
                "Hotkey manager not initialized".to_string(),
            ));
        }

        Ok(())
    }

    async fn start_event_listener(&mut self) -> Result<()> {
        let event_tx = self.event_tx.clone();
        let copy_hotkey = self.copy_hotkey.expect("Copy hotkey should be registered");
        let paste_hotkey = self
            .paste_hotkey
            .expect("Paste hotkey should be registered");
        let is_running = self.is_running.clone();

        is_running.store(true, Ordering::SeqCst);

        info!("🎹 Starting event listener...");
        info!("🎹 Listening for copy hotkey ID: {}", copy_hotkey.id());
        info!("🎹 Listening for paste hotkey ID: {}", paste_hotkey.id());

        let hotkey_thread = std::thread::spawn(move || {
            info!("🎹 Hotkey listener thread started");

            #[cfg(target_os = "windows")]
            {
                // Initialize COM for Windows
                unsafe {
                    use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
                    let com_result = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
                    info!("🪟 COM initialization result: {:?}", com_result);
                }
            }

            let receiver = GlobalHotKeyEvent::receiver();
            info!("🎹 Event receiver created");

            let mut event_count = 0;
            while is_running.load(Ordering::SeqCst) {
                // Process Windows messages on Windows
                #[cfg(target_os = "windows")]
                {
                    unsafe {
                        use windows::Win32::UI::WindowsAndMessaging::{
                            DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
                            WM_QUIT,
                        };

                        let mut msg = MSG::default();
                        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                            if msg.message == WM_QUIT {
                                info!("🪟 Received WM_QUIT message");
                                is_running.store(false, Ordering::SeqCst);
                                break;
                            }
                            TranslateMessage(&msg);
                            DispatchMessageW(&msg);
                        }
                    }
                }

                // Check for hotkey events
                match receiver.try_recv() {
                    Ok(event) => {
                        event_count += 1;
                        info!(
                            "🎹 Event #{} received - ID: {}, State: {:?}",
                            event_count, event.id, event.state
                        );

                        if event.state == global_hotkey::HotKeyState::Pressed {
                            let hotkey_event = if event.id == copy_hotkey.id() {
                                info!("🎹 COPY HOTKEY DETECTED! (ID: {})", event.id);
                                Some(HotkeyEvent::CopyFiles)
                            } else if event.id == paste_hotkey.id() {
                                info!("🎹 PASTE HOTKEY DETECTED! (ID: {})", event.id);
                                Some(HotkeyEvent::PasteFiles)
                            } else {
                                warn!(
                                    "🎹 Unknown hotkey ID: {} (expected copy: {} or paste: {})",
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
                                } else {
                                    info!("✅ Hotkey event sent successfully");
                                }
                            }
                        } else {
                            debug!("🎹 Ignoring hotkey release event");
                        }
                    }
                    Err(crossbeam_channel::TryRecvError::Empty) => {
                        // No events, continue
                    }
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        warn!("🎹 Hotkey event receiver disconnected");
                        break;
                    }
                }

                std::thread::sleep(std::time::Duration::from_millis(10));
            }

            #[cfg(target_os = "windows")]
            {
                unsafe {
                    use windows::Win32::System::Com::CoUninitialize;
                    CoUninitialize();
                }
            }

            info!(
                "🎹 Hotkey listener thread stopped (processed {} events)",
                event_count
            );
        });

        self._hotkey_thread = Some(hotkey_thread);

        // Give the system time to initialize
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        info!("✅ Hotkey listener initialized and ready");

        Ok(())
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

        self.is_running.store(false, Ordering::SeqCst);

        if let Some(manager) = &self.manager {
            if let Some(copy_hotkey) = self.copy_hotkey {
                if let Err(e) = manager.unregister(copy_hotkey) {
                    warn!("❌ Failed to unregister copy hotkey: {}", e);
                } else {
                    info!("✅ Copy hotkey unregistered");
                }
            }

            if let Some(paste_hotkey) = self.paste_hotkey {
                if let Err(e) = manager.unregister(paste_hotkey) {
                    warn!("❌ Failed to unregister paste hotkey: {}", e);
                } else {
                    info!("✅ Paste hotkey unregistered");
                }
            }
        }

        if let Some(thread) = self._hotkey_thread.take() {
            std::thread::spawn(move || {
                let _ = thread.join();
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
