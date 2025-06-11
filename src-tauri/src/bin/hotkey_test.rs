use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyEvent, GlobalHotKeyManager,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

fn main() {
    println!("🧪 STANDALONE HOTKEY TEST WITH MESSAGE PUMP");
    println!("Platform: {}", std::env::consts::OS);

    // Test manager creation
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

    // Test registration
    let hotkeys = vec![
        (
            "Ctrl+Alt+F1",
            HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::F1),
        ),
        (
            "Ctrl+Alt+F2",
            HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::F2),
        ),
        (
            "Ctrl+Alt+Y",
            HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyY),
        ),
        (
            "Ctrl+Alt+I",
            HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyI),
        ),
    ];

    let mut registered = Vec::new();
    for (name, hotkey) in &hotkeys {
        match manager.register(*hotkey) {
            Ok(()) => {
                println!("✅ Registered: {} (ID: {})", name, hotkey.id());
                registered.push((name, *hotkey));
            }
            Err(e) => {
                println!("❌ Failed: {} - {}", name, e);
            }
        }
    }

    if registered.is_empty() {
        println!("❌ NO HOTKEYS REGISTERED!");
        return;
    }

    println!("\n🎯 Starting Windows message loop...");
    println!("Press any registered hotkey, or Ctrl+C to exit");
    println!("Registered hotkeys:");
    for (name, _) in &registered {
        println!("  - {}", name);
    }

    let running = Arc::new(AtomicBool::new(true));
    let running_clone = running.clone();

    // Handle Ctrl+C
    ctrlc::set_handler(move || {
        println!("\n🛑 Received Ctrl+C, shutting down...");
        running_clone.store(false, Ordering::SeqCst);
    })
    .expect("Error setting Ctrl+C handler");

    let receiver = GlobalHotKeyEvent::receiver();
    let mut event_count = 0;

    // CRITICAL: Windows Message Loop
    while running.load(Ordering::SeqCst) {
        // Pump Windows messages first
        #[cfg(target_os = "windows")]
        pump_windows_messages();

        // Then check for hotkey events
        match receiver.try_recv() {
            Ok(event) => {
                event_count += 1;
                println!(
                    "🎯 EVENT #{}: ID={}, State={:?}",
                    event_count, event.id, event.state
                );

                // Identify which hotkey
                for (name, hotkey) in &registered {
                    if event.id == hotkey.id() {
                        println!(
                            "   └─ {} ({})",
                            name,
                            if event.state == global_hotkey::HotKeyState::Pressed {
                                "PRESSED"
                            } else {
                                "RELEASED"
                            }
                        );
                        break;
                    }
                }
            }
            Err(crossbeam_channel::TryRecvError::Empty) => {
                // No events, continue pumping messages
            }
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                println!("❌ Event receiver disconnected");
                break;
            }
        }

        // Small delay to prevent CPU spinning
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    println!("\nTotal events received: {}", event_count);

    // Cleanup
    println!("\n🧹 Cleaning up...");
    for (name, hotkey) in &registered {
        match manager.unregister(*hotkey) {
            Ok(()) => println!("✅ Unregistered: {}", name),
            Err(e) => println!("❌ Failed to unregister {}: {}", name, e),
        }
    }

    println!("✅ Test complete");
}

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
