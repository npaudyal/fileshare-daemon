use crate::Result;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

#[derive(Debug, Clone)]
pub enum HotkeyEvent {
    CopyFiles,  // Cmd+Shift+Y (macOS) or Ctrl+Alt+Y (Windows) or Ctrl+Shift+Y (Linux)
    PasteFiles, // Cmd+Shift+I (macOS) or Ctrl+Alt+I (Windows) or Ctrl+Shift+I (Linux)
}

pub struct HotkeyManager {
    event_tx: mpsc::UnboundedSender<HotkeyEvent>,
    event_rx: mpsc::UnboundedReceiver<HotkeyEvent>,
    is_running: Arc<AtomicBool>,
    platform_handle: Option<std::thread::JoinHandle<()>>,
}

impl HotkeyManager {
    pub fn new() -> Result<Self> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();

        // Log platform-specific hotkey combinations
        let (copy_key_str, paste_key_str) = if cfg!(target_os = "macos") {
            ("Cmd+Shift+Y", "Cmd+Shift+I")
        } else if cfg!(target_os = "windows") {
            ("Ctrl+Alt+Y", "Ctrl+Alt+I")
        } else {
            ("Ctrl+Shift+Y", "Ctrl+Shift+I")
        };

        info!("🎹 Hotkey combinations for this platform:");
        info!("  📋 Copy: {}", copy_key_str);
        info!("  📁 Paste: {}", paste_key_str);

        Ok(Self {
            event_tx,
            event_rx,
            is_running: Arc::new(AtomicBool::new(false)),
            platform_handle: None,
        })
    }

    pub async fn start(&mut self) -> Result<()> {
        info!("🎹 Starting platform-specific hotkey manager...");

        self.is_running.store(true, Ordering::SeqCst);

        // Create platform-specific hotkey thread
        let event_tx = self.event_tx.clone();
        let is_running = self.is_running.clone();

        let handle = std::thread::spawn(move || {
            Self::platform_hotkey_thread(event_tx, is_running);
        });

        self.platform_handle = Some(handle);

        // Give the platform time to set up
        tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;

        info!("✅ Platform-specific hotkey system initialized");
        Ok(())
    }

    #[cfg(target_os = "windows")]
    fn platform_hotkey_thread(
        event_tx: mpsc::UnboundedSender<HotkeyEvent>,
        is_running: Arc<AtomicBool>,
    ) {
        use global_hotkey::{
            hotkey::{Code, HotKey, Modifiers},
            GlobalHotKeyEvent, GlobalHotKeyManager,
        };

        info!("🎹 Windows hotkey thread started");

        // CRITICAL: Create hotkey manager on THIS thread
        let manager = match GlobalHotKeyManager::new() {
            Ok(manager) => {
                info!("✅ Windows hotkey manager created on dedicated thread");
                manager
            }
            Err(e) => {
                error!("❌ Failed to create Windows hotkey manager: {}", e);
                return;
            }
        };

        // Register hotkeys on THIS thread
        let copy_hotkey = HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyY);
        let paste_hotkey = HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyI);

        if let Err(e) = manager.register(copy_hotkey) {
            error!("❌ Failed to register copy hotkey: {}", e);
            return;
        }

        if let Err(e) = manager.register(paste_hotkey) {
            error!("❌ Failed to register paste hotkey: {}", e);
            return;
        }

        info!("✅ Windows hotkeys registered: Ctrl+Alt+Y (copy), Ctrl+Alt+I (paste)");

        // Get event receiver on THIS thread
        let receiver = GlobalHotKeyEvent::receiver();

        info!("🎹 Starting Windows event loop with message pump");

        // CRITICAL: Run Win32 event loop on THIS SAME thread
        while is_running.load(Ordering::SeqCst) {
            // STEP 1: Pump Windows messages (CRITICAL for hotkeys to work)
            Self::pump_windows_messages();

            // STEP 2: Check for hotkey events
            match receiver.try_recv() {
                Ok(event) => {
                    if event.state == global_hotkey::HotKeyState::Pressed {
                        info!("🎯 Windows hotkey event: ID={}", event.id);

                        let hotkey_event = if event.id == copy_hotkey.id() {
                            info!("🎹 Copy hotkey detected on Windows!");
                            Some(HotkeyEvent::CopyFiles)
                        } else if event.id == paste_hotkey.id() {
                            info!("🎹 Paste hotkey detected on Windows!");
                            Some(HotkeyEvent::PasteFiles)
                        } else {
                            None
                        };

                        if let Some(hotkey_event) = hotkey_event {
                            if let Err(e) = event_tx.send(hotkey_event) {
                                error!("❌ Failed to send Windows hotkey event: {}", e);
                                break;
                            }
                        }
                    }
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    // No events, continue
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    warn!("🎹 Windows hotkey receiver disconnected");
                    break;
                }
            }

            // Small delay to prevent CPU spinning
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        // Cleanup
        let _ = manager.unregister(copy_hotkey);
        let _ = manager.unregister(paste_hotkey);
        info!("🎹 Windows hotkey thread ended");
    }

    #[cfg(target_os = "macos")]
    fn platform_hotkey_thread(
        event_tx: mpsc::UnboundedSender<HotkeyEvent>,
        is_running: Arc<AtomicBool>,
    ) {
        use global_hotkey::{
            hotkey::{Code, HotKey, Modifiers},
            GlobalHotKeyEvent, GlobalHotKeyManager,
        };

        info!("🎹 macOS hotkey thread started");

        let manager = match GlobalHotKeyManager::new() {
            Ok(manager) => {
                info!("✅ macOS hotkey manager created");
                manager
            }
            Err(e) => {
                error!("❌ Failed to create macOS hotkey manager: {}", e);
                return;
            }
        };

        let copy_hotkey = HotKey::new(Some(Modifiers::META | Modifiers::SHIFT), Code::KeyY);
        let paste_hotkey = HotKey::new(Some(Modifiers::META | Modifiers::SHIFT), Code::KeyI);

        if let Err(e) = manager.register(copy_hotkey) {
            error!("❌ Failed to register copy hotkey: {}", e);
            return;
        }

        if let Err(e) = manager.register(paste_hotkey) {
            error!("❌ Failed to register paste hotkey: {}", e);
            return;
        }

        info!("✅ macOS hotkeys registered: Cmd+Shift+Y (copy), Cmd+Shift+I (paste)");

        let receiver = GlobalHotKeyEvent::receiver();

        while is_running.load(Ordering::SeqCst) {
            match receiver.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(event) => {
                    if event.state == global_hotkey::HotKeyState::Pressed {
                        let hotkey_event = if event.id == copy_hotkey.id() {
                            info!("🎹 Copy hotkey detected on macOS!");
                            Some(HotkeyEvent::CopyFiles)
                        } else if event.id == paste_hotkey.id() {
                            info!("🎹 Paste hotkey detected on macOS!");
                            Some(HotkeyEvent::PasteFiles)
                        } else {
                            None
                        };

                        if let Some(hotkey_event) = hotkey_event {
                            if let Err(e) = event_tx.send(hotkey_event) {
                                error!("❌ Failed to send macOS hotkey event: {}", e);
                                break;
                            }
                        }
                    }
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    // Normal timeout, continue
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    warn!("🎹 macOS hotkey receiver disconnected");
                    break;
                }
            }
        }

        let _ = manager.unregister(copy_hotkey);
        let _ = manager.unregister(paste_hotkey);
        info!("🎹 macOS hotkey thread ended");
    }

    #[cfg(target_os = "linux")]
    fn platform_hotkey_thread(
        event_tx: mpsc::UnboundedSender<HotkeyEvent>,
        is_running: Arc<AtomicBool>,
    ) {
        use global_hotkey::{
            hotkey::{Code, HotKey, Modifiers},
            GlobalHotKeyEvent, GlobalHotKeyManager,
        };

        info!("🎹 Linux hotkey thread started");

        let manager = match GlobalHotKeyManager::new() {
            Ok(manager) => {
                info!("✅ Linux hotkey manager created");
                manager
            }
            Err(e) => {
                error!("❌ Failed to create Linux hotkey manager: {}", e);
                return;
            }
        };

        let copy_hotkey = HotKey::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyY);
        let paste_hotkey = HotKey::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyI);

        if let Err(e) = manager.register(copy_hotkey) {
            error!("❌ Failed to register copy hotkey: {}", e);
            return;
        }

        if let Err(e) = manager.register(paste_hotkey) {
            error!("❌ Failed to register paste hotkey: {}", e);
            return;
        }

        info!("✅ Linux hotkeys registered: Ctrl+Shift+Y (copy), Ctrl+Shift+I (paste)");

        let receiver = GlobalHotKeyEvent::receiver();

        while is_running.load(Ordering::SeqCst) {
            match receiver.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(event) => {
                    if event.state == global_hotkey::HotKeyState::Pressed {
                        let hotkey_event = if event.id == copy_hotkey.id() {
                            info!("🎹 Copy hotkey detected on Linux!");
                            Some(HotkeyEvent::CopyFiles)
                        } else if event.id == paste_hotkey.id() {
                            info!("🎹 Paste hotkey detected on Linux!");
                            Some(HotkeyEvent::PasteFiles)
                        } else {
                            None
                        };

                        if let Some(hotkey_event) = hotkey_event {
                            if let Err(e) = event_tx.send(hotkey_event) {
                                error!("❌ Failed to send Linux hotkey event: {}", e);
                                break;
                            }
                        }
                    }
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    // Normal timeout, continue
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    warn!("🎹 Linux hotkey receiver disconnected");
                    break;
                }
            }
        }

        let _ = manager.unregister(copy_hotkey);
        let _ = manager.unregister(paste_hotkey);
        info!("🎹 Linux hotkey thread ended");
    }

    // CRITICAL: Windows message pump function
    #[cfg(target_os = "windows")]
    fn pump_windows_messages() {
        use std::ptr;

        #[repr(C)]
        struct MSG {
            hwnd: *mut std::ffi::c_void,
            message: u32,
            wparam: usize,
            lparam: isize,
            time: u32,
            pt: (i32, i32),
        }

        extern "system" {
            fn PeekMessageW(
                lpmsg: *mut MSG,
                hwnd: *mut std::ffi::c_void,
                wmsgfiltermin: u32,
                wmsgfiltermax: u32,
                wremovemsg: u32,
            ) -> i32;
            fn TranslateMessage(lpmsg: *const MSG) -> i32;
            fn DispatchMessageW(lpmsg: *const MSG) -> isize;
        }

        const PM_REMOVE: u32 = 0x0001;

        unsafe {
            let mut msg: MSG = std::mem::zeroed();

            // Process all available messages
            while PeekMessageW(&mut msg, ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    fn pump_windows_messages() {
        // No-op on non-Windows platforms
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
        info!("🛑 Stopping platform-specific hotkey manager");

        // Signal the thread to stop
        self.is_running.store(false, Ordering::SeqCst);

        // Wait for thread to finish
        if let Some(_handle) = self.platform_handle.take() {
            // Give it a moment to stop gracefully
            std::thread::sleep(std::time::Duration::from_millis(100));
            // The thread should stop on its own due to is_running flag
        }

        info!("✅ Platform-specific hotkey manager stopped");
        Ok(())
    }
}

impl Drop for HotkeyManager {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
