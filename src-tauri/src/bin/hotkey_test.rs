use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyEvent, GlobalHotKeyManager,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

fn main() {
    println!("🧪 STANDALONE HOTKEY TEST");
    println!("Platform: {}", std::env::consts::OS);
    println!("Architecture: {}", std::env::consts::ARCH);

    // Test outside of Tauri environment
    test_basic_hotkeys();
}

fn test_basic_hotkeys() {
    println!("\n--- BASIC HOTKEY TEST ---");

    // Create manager
    let manager = match GlobalHotKeyManager::new() {
        Ok(m) => {
            println!("✅ Manager created");
            m
        }
        Err(e) => {
            println!("❌ Manager creation failed: {}", e);
            return;
        }
    };

    // Define test hotkeys
    let hotkeys = vec![
        (
            "Ctrl+Alt+F1",
            HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::F1),
        ),
        (
            "Ctrl+Alt+F2",
            HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::F2),
        ),
    ];

    // Register hotkeys
    let mut registered = Vec::new();
    for (name, hotkey) in &hotkeys {
        match manager.register(*hotkey) {
            Ok(()) => {
                println!("✅ Registered: {} (ID: {})", name, hotkey.id());
                registered.push((name, *hotkey));
            }
            Err(e) => {
                println!("❌ Failed to register {}: {}", name, e);
            }
        }
    }

    if registered.is_empty() {
        println!("❌ No hotkeys registered!");
        return;
    }

    // Listen for events
    println!("\n🎯 Listening for hotkey events...");
    println!("Press Ctrl+Alt+F1 or Ctrl+Alt+F2, or Ctrl+C to exit");

    let receiver = GlobalHotKeyEvent::receiver();
    let running = Arc::new(AtomicBool::new(true));
    let running_clone = running.clone();

    // Handle Ctrl+C
    ctrlc::set_handler(move || {
        println!("\n🛑 Received Ctrl+C, shutting down...");
        running_clone.store(false, Ordering::SeqCst);
    })
    .expect("Error setting Ctrl+C handler");

    let mut event_count = 0;
    while running.load(Ordering::SeqCst) {
        match receiver.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(event) => {
                event_count += 1;
                println!(
                    "🎯 Event #{}: ID={}, State={:?}",
                    event_count, event.id, event.state
                );

                // Identify the hotkey
                for (name, hotkey) in &registered {
                    if event.id == hotkey.id() {
                        println!("   └─ {}", name);
                        if event.state == global_hotkey::HotKeyState::Pressed {
                            println!("   └─ PRESSED!");
                        }
                        break;
                    }
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                // Normal timeout, continue
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                println!("❌ Event receiver disconnected");
                break;
            }
        }
    }

    // Cleanup
    println!("\n🧹 Cleaning up...");
    for (name, hotkey) in &registered {
        match manager.unregister(*hotkey) {
            Ok(()) => println!("✅ Unregistered: {}", name),
            Err(e) => println!("❌ Failed to unregister {}: {}", name, e),
        }
    }

    println!("Total events received: {}", event_count);
    println!("✅ Test complete");
}
