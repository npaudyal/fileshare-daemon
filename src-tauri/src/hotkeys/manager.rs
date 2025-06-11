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
    event_tx: mpsc::UnboundedSender<HotkeyEvent>,
    event_rx: mpsc::UnboundedReceiver<HotkeyEvent>,
    is_running: Arc<AtomicBool>,
    platform_handle: Option<PlatformHotkeyHandle>,
}

// Platform-specific handle to manage the hotkey system
enum PlatformHotkeyHandle {
    #[cfg(target_os = "windows")]
    Windows(std::thread::JoinHandle<()>),
    #[cfg(target_os = "macos")]
    MacOS(std::thread::JoinHandle<()>),
    #[cfg(target_os = "linux")]
    Linux(std::thread::JoinHandle<()>),
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

        #[cfg(target_os = "windows")]
        {
            let handle = self.start_windows_hotkeys().await?;
            self.platform_handle = Some(PlatformHotkeyHandle::Windows(handle));
        }

        #[cfg(target_os = "macos")]
        {
            let handle = self.start_macos_hotkeys().await?;
            self.platform_handle = Some(PlatformHotkeyHandle::MacOS(handle));
        }

        #[cfg(target_os = "linux")]
        {
            let handle = self.start_linux_hotkeys().await?;
            self.platform_handle = Some(PlatformHotkeyHandle::Linux(handle));
        }

        // Give the platform time to set up
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        info!("✅ Platform-specific hotkey system initialized");
        Ok(())
    }

    #[cfg(target_os = "windows")]
    async fn start_windows_hotkeys(&self) -> Result<std::thread::JoinHandle<()>> {
        info!("🎹 Starting Windows-specific hotkey system...");

        let event_tx = self.event_tx.clone();
        let is_running = self.is_running.clone();

        let handle = std::thread::spawn(move || {
            info!("🎹 Windows hotkey thread started");

            // CREATE THE MANAGER ON THE WINDOWS THREAD - THIS IS CRITICAL
            let manager = match GlobalHotKeyManager::new() {
                Ok(m) => {
                    info!("✅ GlobalHotKeyManager created on Windows thread");
                    m
                }
                Err(e) => {
                    error!(
                        "❌ Failed to create hotkey manager on Windows thread: {}",
                        e
                    );
                    return;
                }
            };

            // Define hotkeys for Windows
            let copy_hotkey = HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyY);
            let paste_hotkey = HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyI);

            // Register hotkeys
            if let Err(e) = manager.register(copy_hotkey) {
                error!("❌ Failed to register copy hotkey on Windows: {}", e);
                // Try fallback
                let fallback_copy =
                    HotKey::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::F1);
                if let Err(e2) = manager.register(fallback_copy) {
                    error!("❌ Failed to register fallback copy hotkey: {}", e2);
                    return;
                } else {
                    info!("✅ Registered fallback copy hotkey: Ctrl+Shift+F1");
                }
            } else {
                info!("✅ Copy hotkey registered: Ctrl+Alt+Y");
            }

            if let Err(e) = manager.register(paste_hotkey) {
                error!("❌ Failed to register paste hotkey on Windows: {}", e);
                // Try fallback
                let fallback_paste =
                    HotKey::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::F2);
                if let Err(e2) = manager.register(fallback_paste) {
                    error!("❌ Failed to register fallback paste hotkey: {}", e2);
                    return;
                } else {
                    info!("✅ Registered fallback paste hotkey: Ctrl+Shift+F2");
                }
            } else {
                info!("✅ Paste hotkey registered: Ctrl+Alt+I");
            }

            // Get the event receiver
            let receiver = GlobalHotKeyEvent::receiver();
            info!("🎹 Windows hotkey event loop starting...");

            // MAIN WINDOWS MESSAGE PUMP LOOP
            while is_running.load(Ordering::SeqCst) {
                match receiver.try_recv() {
                    Ok(event) => {
                        if event.state == global_hotkey::HotKeyState::Pressed {
                            info!("🎹 Windows hotkey pressed: ID={}", event.id);

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
                                if let Err(e) = event_tx.send(hotkey_event) {
                                    error!("❌ Failed to send hotkey event: {}", e);
                                    break;
                                }
                            }
                        }
                    }
                    Err(crossbeam_channel::TryRecvError::Empty) => {
                        // No events, sleep briefly and pump messages
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        warn!("🎹 Hotkey event receiver disconnected");
                        break;
                    }
                }

                // CRITICAL: Let Windows process messages
                std::thread::sleep(std::time::Duration::from_millis(1));
            }

            // Cleanup
            let _ = manager.unregister(copy_hotkey);
            let _ = manager.unregister(paste_hotkey);
            info!("🎹 Windows hotkey thread stopped");
        });

        Ok(handle)
    }

    #[cfg(target_os = "macos")]
    async fn start_macos_hotkeys(&self) -> Result<std::thread::JoinHandle<()>> {
        info!("🎹 Starting macOS-specific hotkey system...");

        let event_tx = self.event_tx.clone();
        let is_running = self.is_running.clone();

        let handle = std::thread::spawn(move || {
            info!("🎹 macOS hotkey thread started");

            // Create manager on macOS thread
            let manager = match GlobalHotKeyManager::new() {
                Ok(m) => {
                    info!("✅ GlobalHotKeyManager created on macOS thread");
                    m
                }
                Err(e) => {
                    error!("❌ Failed to create hotkey manager on macOS thread: {}", e);
                    return;
                }
            };

            // Define hotkeys for macOS
            let copy_hotkey = HotKey::new(Some(Modifiers::META | Modifiers::SHIFT), Code::KeyY);
            let paste_hotkey = HotKey::new(Some(Modifiers::META | Modifiers::SHIFT), Code::KeyI);

            // Register hotkeys
            if let Err(e) = manager.register(copy_hotkey) {
                error!("❌ Failed to register copy hotkey on macOS: {}", e);
                return;
            } else {
                info!("✅ Copy hotkey registered: Cmd+Shift+Y");
            }

            if let Err(e) = manager.register(paste_hotkey) {
                error!("❌ Failed to register paste hotkey on macOS: {}", e);
                return;
            } else {
                info!("✅ Paste hotkey registered: Cmd+Shift+I");
            }

            // Get the event receiver
            let receiver = GlobalHotKeyEvent::receiver();
            info!("🎹 macOS hotkey event loop starting...");

            // Main event loop
            while is_running.load(Ordering::SeqCst) {
                match receiver.try_recv() {
                    Ok(event) => {
                        if event.state == global_hotkey::HotKeyState::Pressed {
                            info!("🎹 macOS hotkey pressed: ID={}", event.id);

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
                                if let Err(e) = event_tx.send(hotkey_event) {
                                    error!("❌ Failed to send hotkey event: {}", e);
                                    break;
                                }
                            }
                        }
                    }
                    Err(crossbeam_channel::TryRecvError::Empty) => {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        warn!("🎹 Hotkey event receiver disconnected");
                        break;
                    }
                }
            }

            // Cleanup
            let _ = manager.unregister(copy_hotkey);
            let _ = manager.unregister(paste_hotkey);
            info!("🎹 macOS hotkey thread stopped");
        });

        Ok(handle)
    }

    #[cfg(target_os = "linux")]
    async fn start_linux_hotkeys(&self) -> Result<std::thread::JoinHandle<()>> {
        info!("🎹 Starting Linux-specific hotkey system...");

        let event_tx = self.event_tx.clone();
        let is_running = self.is_running.clone();

        let handle = std::thread::spawn(move || {
            info!("🎹 Linux hotkey thread started");

            // Create manager on Linux thread
            let manager = match GlobalHotKeyManager::new() {
                Ok(m) => {
                    info!("✅ GlobalHotKeyManager created on Linux thread");
                    m
                }
                Err(e) => {
                    error!("❌ Failed to create hotkey manager on Linux thread: {}", e);
                    return;
                }
            };

            // Define hotkeys for Linux
            let copy_hotkey = HotKey::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyY);
            let paste_hotkey = HotKey::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyI);

            // Register hotkeys
            if let Err(e) = manager.register(copy_hotkey) {
                error!("❌ Failed to register copy hotkey on Linux: {}", e);
                return;
            } else {
                info!("✅ Copy hotkey registered: Ctrl+Shift+Y");
            }

            if let Err(e) = manager.register(paste_hotkey) {
                error!("❌ Failed to register paste hotkey on Linux: {}", e);
                return;
            } else {
                info!("✅ Paste hotkey registered: Ctrl+Shift+I");
            }

            // Get the event receiver
            let receiver = GlobalHotKeyEvent::receiver();
            info!("🎹 Linux hotkey event loop starting...");

            // Main event loop
            while is_running.load(Ordering::SeqCst) {
                match receiver.try_recv() {
                    Ok(event) => {
                        if event.state == global_hotkey::HotKeyState::Pressed {
                            info!("🎹 Linux hotkey pressed: ID={}", event.id);

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
                                if let Err(e) = event_tx.send(hotkey_event) {
                                    error!("❌ Failed to send hotkey event: {}", e);
                                    break;
                                }
                            }
                        }
                    }
                    Err(crossbeam_channel::TryRecvError::Empty) => {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        warn!("🎹 Hotkey event receiver disconnected");
                        break;
                    }
                }
            }

            // Cleanup
            let _ = manager.unregister(copy_hotkey);
            let _ = manager.unregister(paste_hotkey);
            info!("🎹 Linux hotkey thread stopped");
        });

        Ok(handle)
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

        // Wait for platform-specific cleanup
        if let Some(handle) = self.platform_handle.take() {
            match handle {
                #[cfg(target_os = "windows")]
                PlatformHotkeyHandle::Windows(thread_handle) => {
                    info!("🛑 Stopping Windows hotkey thread");
                    // Give it a moment to stop gracefully
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    // The thread should stop on its own due to is_running flag
                }
                #[cfg(target_os = "macos")]
                PlatformHotkeyHandle::MacOS(thread_handle) => {
                    info!("🛑 Stopping macOS hotkey thread");
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                #[cfg(target_os = "linux")]
                PlatformHotkeyHandle::Linux(thread_handle) => {
                    info!("🛑 Stopping Linux hotkey thread");
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
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
