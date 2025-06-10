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
        let manager = GlobalHotKeyManager::new().map_err(|e| {
            error!("❌ Failed to create hotkey manager: {}", e);
            FileshareError::Unknown(format!("Failed to create hotkey manager: {}", e))
        })?;

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
            // Platform-specific key combinations
            let (copy_modifiers, paste_modifiers, copy_key_str, paste_key_str) =
                if cfg!(target_os = "macos") {
                    (
                        Modifiers::META | Modifiers::SHIFT, // Cmd+Shift on macOS
                        Modifiers::META | Modifiers::SHIFT,
                        "Cmd+Shift+Y",
                        "Cmd+Shift+I",
                    )
                } else if cfg!(target_os = "windows") {
                    (
                        Modifiers::CONTROL | Modifiers::SHIFT, // Ctrl+Shift on Windows
                        Modifiers::CONTROL | Modifiers::SHIFT,
                        "Ctrl+Shift+Y",
                        "Ctrl+Shift+I",
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

            // Try to register hotkeys with fallbacks
            let (final_copy_hotkey, copy_combo) =
                self.register_copy_hotkey(manager, copy_hotkey, copy_key_str)?;
            let (final_paste_hotkey, paste_combo) =
                self.register_paste_hotkey(manager, paste_hotkey, paste_key_str)?;

            self.copy_hotkey = Some(final_copy_hotkey);
            self.paste_hotkey = Some(final_paste_hotkey);

            info!("✅ Hotkeys registered successfully:");
            info!("   📋 Copy: {}", copy_combo);
            info!("   📁 Paste: {}", paste_combo);

            // Start platform-specific listener
            self.start_event_listener().await?;
        } else {
            return Err(FileshareError::Unknown(
                "Hotkey manager not initialized".to_string(),
            ));
        }

        Ok(())
    }

    fn register_copy_hotkey(
        &self,
        manager: &GlobalHotKeyManager,
        primary: HotKey,
        primary_desc: &str,
    ) -> Result<(HotKey, String)> {
        // Try primary hotkey first
        match manager.register(primary) {
            Ok(()) => {
                info!("✅ Copy hotkey registered: {}", primary_desc);
                return Ok((primary, primary_desc.to_string()));
            }
            Err(e) => {
                warn!(
                    "❌ Failed to register primary copy hotkey {}: {}",
                    primary_desc, e
                );
            }
        }

        // Fallback combinations for copy
        let copy_fallbacks = if cfg!(target_os = "windows") {
            vec![
                (
                    Modifiers::CONTROL | Modifiers::ALT,
                    Code::KeyY,
                    "Ctrl+Alt+Y",
                ),
                (Modifiers::SHIFT | Modifiers::ALT, Code::KeyY, "Shift+Alt+Y"),
                (
                    Modifiers::CONTROL | Modifiers::SHIFT,
                    Code::F9,
                    "Ctrl+Shift+F9",
                ),
                (Modifiers::CONTROL | Modifiers::ALT, Code::F9, "Ctrl+Alt+F9"),
            ]
        } else {
            vec![
                (
                    Modifiers::CONTROL | Modifiers::ALT,
                    Code::KeyY,
                    "Ctrl+Alt+Y",
                ),
                (Modifiers::SHIFT | Modifiers::ALT, Code::KeyY, "Shift+Alt+Y"),
                (
                    Modifiers::CONTROL | Modifiers::SHIFT,
                    Code::F9,
                    "Ctrl+Shift+F9",
                ),
            ]
        };

        for (modifiers, code, description) in copy_fallbacks {
            let hotkey = HotKey::new(Some(modifiers), code);
            match manager.register(hotkey) {
                Ok(()) => {
                    info!("✅ Copy hotkey registered with fallback: {}", description);
                    return Ok((hotkey, description.to_string()));
                }
                Err(e) => {
                    debug!("⚠️ Failed to register copy fallback {}: {}", description, e);
                }
            }
        }

        Err(FileshareError::Unknown(
            "Failed to register copy hotkey with any combination".to_string(),
        ))
    }

    fn register_paste_hotkey(
        &self,
        manager: &GlobalHotKeyManager,
        primary: HotKey,
        primary_desc: &str,
    ) -> Result<(HotKey, String)> {
        // Try primary hotkey first
        match manager.register(primary) {
            Ok(()) => {
                info!("✅ Paste hotkey registered: {}", primary_desc);
                return Ok((primary, primary_desc.to_string()));
            }
            Err(e) => {
                warn!(
                    "❌ Failed to register primary paste hotkey {}: {}",
                    primary_desc, e
                );
            }
        }

        // Fallback combinations for paste
        let paste_fallbacks = if cfg!(target_os = "windows") {
            vec![
                (
                    Modifiers::CONTROL | Modifiers::ALT,
                    Code::KeyI,
                    "Ctrl+Alt+I",
                ),
                (Modifiers::SHIFT | Modifiers::ALT, Code::KeyI, "Shift+Alt+I"),
                (
                    Modifiers::CONTROL | Modifiers::SHIFT,
                    Code::F8,
                    "Ctrl+Shift+F8",
                ),
                (Modifiers::CONTROL | Modifiers::ALT, Code::F8, "Ctrl+Alt+F8"),
            ]
        } else {
            vec![
                (
                    Modifiers::CONTROL | Modifiers::ALT,
                    Code::KeyI,
                    "Ctrl+Alt+I",
                ),
                (Modifiers::SHIFT | Modifiers::ALT, Code::KeyI, "Shift+Alt+I"),
                (
                    Modifiers::CONTROL | Modifiers::SHIFT,
                    Code::F8,
                    "Ctrl+Shift+F8",
                ),
            ]
        };

        for (modifiers, code, description) in paste_fallbacks {
            let hotkey = HotKey::new(Some(modifiers), code);
            match manager.register(hotkey) {
                Ok(()) => {
                    info!("✅ Paste hotkey registered with fallback: {}", description);
                    return Ok((hotkey, description.to_string()));
                }
                Err(e) => {
                    debug!(
                        "⚠️ Failed to register paste fallback {}: {}",
                        description, e
                    );
                }
            }
        }

        Err(FileshareError::Unknown(
            "Failed to register paste hotkey with any combination".to_string(),
        ))
    }

    async fn start_event_listener(&mut self) -> Result<()> {
        let event_tx = self.event_tx.clone();
        let copy_hotkey = self.copy_hotkey.expect("Copy hotkey should be registered");
        let paste_hotkey = self
            .paste_hotkey
            .expect("Paste hotkey should be registered");
        let is_running = self.is_running.clone();

        is_running.store(true, Ordering::SeqCst);

        // Platform-specific listener implementations
        #[cfg(target_os = "windows")]
        {
            info!("🎹 Starting Windows hotkey listener with proper message pump...");
            let hotkey_thread = std::thread::spawn(move || {
                Self::windows_message_pump_listener(
                    event_tx,
                    copy_hotkey,
                    paste_hotkey,
                    is_running,
                );
            });
            self._hotkey_thread = Some(hotkey_thread);
        }

        #[cfg(not(target_os = "windows"))]
        {
            info!("🎹 Starting cross-platform hotkey listener...");
            let hotkey_thread = std::thread::spawn(move || {
                Self::cross_platform_listener(event_tx, copy_hotkey, paste_hotkey, is_running);
            });
            self._hotkey_thread = Some(hotkey_thread);
        }

        // Give the system time to initialize
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        info!("✅ Hotkey listener initialized");

        Ok(())
    }

    #[cfg(target_os = "windows")]
    fn windows_message_pump_listener(
        event_tx: mpsc::UnboundedSender<HotkeyEvent>,
        copy_hotkey: HotKey,
        paste_hotkey: HotKey,
        is_running: Arc<AtomicBool>,
    ) {
        info!("🎹 Windows message pump listener thread started");

        // Initialize COM for this thread
        unsafe {
            use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }

        let receiver = GlobalHotKeyEvent::receiver();

        // Windows message pump implementation
        loop {
            if !is_running.load(Ordering::SeqCst) {
                break;
            }

            // Process Windows messages
            unsafe {
                use windows::Win32::UI::WindowsAndMessaging::{
                    DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE, WM_QUIT,
                };

                let mut msg = MSG::default();
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    if msg.message == WM_QUIT {
                        is_running.store(false, Ordering::SeqCst);
                        break;
                    }
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }

            // Check for hotkey events
            match receiver.try_recv() {
                Ok(event) => {
                    if event.state == global_hotkey::HotKeyState::Pressed {
                        info!(
                            "🎹 Windows hotkey event: ID={}, State={:?}",
                            event.id, event.state
                        );

                        let hotkey_event = if event.id == copy_hotkey.id() {
                            info!("🎹 Copy hotkey detected! (ID: {})", event.id);
                            Some(HotkeyEvent::CopyFiles)
                        } else if event.id == paste_hotkey.id() {
                            info!("🎹 Paste hotkey detected! (ID: {})", event.id);
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
                    }
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    // No hotkey events, continue message pump
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    warn!("🎹 Hotkey event receiver disconnected");
                    break;
                }
            }

            // Small sleep to prevent CPU spinning
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        // Cleanup COM
        unsafe {
            use windows::Win32::System::Com::CoUninitialize;
            CoUninitialize();
        }

        info!("🎹 Windows message pump listener stopped");
    }

    #[cfg(not(target_os = "windows"))]
    fn cross_platform_listener(
        event_tx: mpsc::UnboundedSender<HotkeyEvent>,
        copy_hotkey: HotKey,
        paste_hotkey: HotKey,
        is_running: Arc<AtomicBool>,
    ) {
        info!("🎹 Cross-platform hotkey listener thread started");

        let receiver = GlobalHotKeyEvent::receiver();

        while is_running.load(Ordering::SeqCst) {
            match receiver.try_recv() {
                Ok(event) => {
                    if event.state == global_hotkey::HotKeyState::Pressed {
                        info!("🎹 Hotkey event: ID={}, State={:?}", event.id, event.state);

                        let hotkey_event = if event.id == copy_hotkey.id() {
                            info!("🎹 Copy hotkey detected!");
                            Some(HotkeyEvent::CopyFiles)
                        } else if event.id == paste_hotkey.id() {
                            info!("🎹 Paste hotkey detected!");
                            Some(HotkeyEvent::PasteFiles)
                        } else {
                            None
                        };

                        if let Some(hotkey_event) = hotkey_event {
                            if let Err(e) = event_tx.send(hotkey_event) {
                                error!("❌ Failed to send hotkey event: {}", e);
                                break;
                            }
                        }
                    }
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    warn!("🎹 Hotkey event receiver disconnected");
                    break;
                }
            }
        }

        info!("🎹 Cross-platform hotkey listener stopped");
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
