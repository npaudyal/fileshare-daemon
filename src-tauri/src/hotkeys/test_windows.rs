#[cfg(target_os = "windows")]
pub fn test_windows_hotkeys() {
    use global_hotkey::{
        hotkey::{Code, HotKey, Modifiers},
        GlobalHotKeyEvent, GlobalHotKeyManager,
    };
    use tracing::{error, info};

    info!("🧪 Testing Windows hotkey registration...");

    let manager = match GlobalHotKeyManager::new() {
        Ok(m) => {
            info!("✅ GlobalHotKeyManager created successfully");
            m
        }
        Err(e) => {
            error!("❌ Failed to create GlobalHotKeyManager: {}", e);
            return;
        }
    };

    // Test different key combinations
    let test_combinations = vec![
        (
            "Ctrl+Alt+Y",
            Modifiers::CONTROL | Modifiers::ALT,
            Code::KeyY,
        ),
        (
            "Ctrl+Shift+Y",
            Modifiers::CONTROL | Modifiers::SHIFT,
            Code::KeyY,
        ),
        ("Win+Y", Modifiers::SUPER, Code::KeyY),
        ("Ctrl+Alt+F1", Modifiers::CONTROL | Modifiers::ALT, Code::F1),
    ];

    for (name, modifiers, code) in test_combinations {
        let hotkey = HotKey::new(Some(modifiers), code);
        match manager.register(hotkey) {
            Ok(()) => {
                info!("✅ Successfully registered: {}", name);
                if let Err(e) = manager.unregister(hotkey) {
                    error!("❌ Failed to unregister {}: {}", name, e);
                }
            }
            Err(e) => {
                error!("❌ Failed to register {}: {}", name, e);
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub fn test_windows_hotkeys() {
    // No-op on non-Windows platforms
}
