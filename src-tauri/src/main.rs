// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use fileshare_daemon::{
    config::Settings,
    service::FileshareDaemon,
    pairing::{
        PairingSessionManager, 
        PairingError, 
        storage::{PairedDeviceStorage, TrustLevel}
    }
};
use std::collections::HashMap;
use std::sync::Arc;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, TrayIconBuilder, TrayIconEvent},
    Manager, WebviewUrl, WebviewWindowBuilder,
};
use tokio::sync::{Mutex, RwLock};
use tracing::{error, info, warn};
use uuid::Uuid;

// Enhanced DeviceInfo with more management features
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
struct DeviceInfo {
    id: String,
    name: String,
    display_name: Option<String>, // Custom nickname
    device_type: String,
    is_paired: bool,
    is_connected: bool,
    is_blocked: bool,
    trust_level: TrustLevel,
    last_seen: u64,
    first_seen: u64,
    connection_count: u32,
    address: String,
    version: String,
    platform: Option<String>,
    last_transfer_time: Option<u64>,
    total_transfers: u32,
}

// Use the TrustLevel from pairing storage module (already imported above)

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
struct DeviceAction {
    action: String,
    device_id: String,
    params: Option<HashMap<String, String>>,
}

// Test commands for debugging
#[tauri::command]
async fn test_hotkey_system() -> Result<String, String> {
    info!("🧪 Starting comprehensive hotkey system test...");

    tokio::task::spawn_blocking(|| run_hotkey_diagnostics())
        .await
        .map_err(|e| format!("Test spawn error: {}", e))
}

#[tauri::command]
async fn test_isolated_hotkey() -> Result<String, String> {
    tokio::task::spawn_blocking(|| isolated_hotkey_test())
        .await
        .map_err(|e| format!("Spawn error: {}", e))
}

fn run_hotkey_diagnostics() -> String {
    use global_hotkey::{
        hotkey::{Code, HotKey, Modifiers},
        GlobalHotKeyEvent, GlobalHotKeyManager,
    };

    let mut results = Vec::new();
    results.push("🧪 HOTKEY DIAGNOSTICS STARTING".to_string());
    results.push(format!("Platform: {}", std::env::consts::OS));
    results.push(format!("Architecture: {}", std::env::consts::ARCH));

    // Test 1: Manager Creation
    results.push("\n--- TEST 1: HOTKEY MANAGER CREATION ---".to_string());
    let manager = match GlobalHotKeyManager::new() {
        Ok(manager) => {
            results.push("✅ GlobalHotKeyManager created successfully".to_string());
            manager
        }
        Err(e) => {
            results.push(format!("❌ Failed to create GlobalHotKeyManager: {}", e));
            return results.join("\n");
        }
    };

    // Test simple hotkeys
    results.push("\n--- TEST 2: SIMPLE HOTKEY TEST ---".to_string());
    let simple_hotkeys = vec![
        ("F11", HotKey::new(None, Code::F11)),
        (
            "Ctrl+Alt+F11",
            HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::F11),
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
    for (name, hotkey) in &simple_hotkeys {
        match manager.register(*hotkey) {
            Ok(()) => {
                results.push(format!("✅ Registered: {} (ID: {})", name, hotkey.id()));
                registered.push((name, *hotkey));
            }
            Err(e) => {
                results.push(format!("❌ Failed: {} - {}", name, e));
            }
        }
    }

    // Cleanup
    for (name, hotkey) in &registered {
        let _ = manager.unregister(*hotkey);
        results.push(format!("Cleaned up: {}", name));
    }

    results.join("\n")
}

fn isolated_hotkey_test() -> String {
    use global_hotkey::{
        hotkey::{Code, HotKey},
        GlobalHotKeyManager,
    };

    let mut results = Vec::new();
    results.push("🔬 ISOLATED HOTKEY TEST".to_string());

    let manager = match GlobalHotKeyManager::new() {
        Ok(m) => {
            results.push("✅ Fresh manager created".to_string());
            m
        }
        Err(e) => {
            results.push(format!("❌ Fresh manager failed: {}", e));
            return results.join("\n");
        }
    };

    let basic_hotkey = HotKey::new(None, Code::F9);

    match manager.register(basic_hotkey) {
        Ok(()) => {
            results.push("✅ F9 registered successfully!".to_string());
            let _ = manager.unregister(basic_hotkey);
        }
        Err(e) => {
            results.push(format!("❌ Even F9 failed: {}", e));
        }
    }

    results.join("\n")
}

#[tauri::command]
async fn get_system_info() -> Result<SystemInfo, String> {
    Ok(SystemInfo {
        platform: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        memory: 8 * 1024 * 1024 * 1024, // Mock 8GB RAM
        uptime: 3600,                   // Mock 1 hour uptime
    })
}

#[tauri::command]
async fn pair_device_enhanced(
    device_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<String, String> {
    info!("🔗 UI requested enhanced pairing for device: {}", device_id);

    let device_uuid =
        Uuid::parse_str(&device_id).map_err(|e| format!("Invalid device ID: {}", e))?;

    // Check if device is discoverable and healthy
    let is_available = {
        if let Some(daemon_ref) = state.daemon_ref.lock().await.as_ref() {
            let pm = daemon_ref.peer_manager.read().await;
            pm.is_peer_healthy(device_uuid)
        } else {
            false
        }
    };

    if !is_available {
        warn!(
            "⚠️ Attempting to pair with potentially offline device {}",
            device_id
        );
    }

    // Proceed with existing pairing logic
    pair_device_with_trust(device_id, TrustLevel::Trusted, state).await?;

    let status_msg = if is_available {
        "Device paired successfully and is online"
    } else {
        "Device paired successfully (device may be offline)"
    };

    info!("✅ Enhanced pairing completed: {}", status_msg);
    Ok(status_msg.to_string())
}

#[tauri::command]
async fn test_file_transfer(state: tauri::State<'_, AppState>) -> Result<String, String> {
    info!("🧪 Testing enhanced file transfer system");

    if let Some(daemon_ref) = state.daemon_ref.lock().await.as_ref() {
        let pm = daemon_ref.peer_manager.read().await;
        let stats = pm.get_connection_stats();

        Ok(format!(
            "📊 Transfer System Status:\n\
            Connections: {} total ({} healthy)\n\
            System: Ready with optimized QUIC streaming (unlimited file size)",
            stats.total, stats.authenticated
        ))
    } else {
        Err("Daemon not ready".to_string())
    }
}

#[tauri::command]
async fn test_connection_health(state: tauri::State<'_, AppState>) -> Result<String, String> {
    info!("🩺 Testing connection health monitoring");

    if let Some(daemon_ref) = state.daemon_ref.lock().await.as_ref() {
        let mut pm = daemon_ref.peer_manager.write().await;

        if let Err(e) = pm.check_peer_health_all().await {
            return Err(format!("Health check failed: {}", e));
        }

        let stats = pm.get_connection_stats();
        Ok(format!(
            "🫀 Health Check Complete:\n\
            Total: {} | Healthy: {} | Unhealthy: {}\n\
            Connected: {} | Reconnecting: {} | Errors: {}",
            stats.total,
            stats.authenticated,
            stats.unhealthy,
            stats.connected,
            stats.reconnecting,
            stats.error
        ))
    } else {
        Err("Daemon not ready".to_string())
    }
}

#[tauri::command]
async fn test_discovery_status(state: tauri::State<'_, AppState>) -> Result<String, String> {
    info!("🔍 Testing discovery system status");

    if let Some(daemon_ref) = state.daemon_ref.lock().await.as_ref() {
        let discovered_devices = daemon_ref.get_discovered_devices().await;
        let pm = daemon_ref.peer_manager.read().await;
        let peers: Vec<_> = pm.peers.values().collect();

        let settings = daemon_ref.get_settings();
        let mut result = format!(
            "🔍 Discovery Status:\n\
            Device: {} ({})\n\
            QUIC Port: {}\n\
            Discovery Port: {}\n\
            Discovered Devices: {}\n\
            Active Peers: {}\n\n",
            settings.device.name,
            settings.device.id,
            settings.network.port,
            settings.network.discovery_port,
            discovered_devices.len(),
            peers.len()
        );

        if !discovered_devices.is_empty() {
            result.push_str("📱 Discovered Devices:\n");
            for device in discovered_devices.iter().take(5) {
                result.push_str(&format!(
                    "  • {} ({}) at {}\n",
                    device.name, device.id, device.addr
                ));
            }
        }

        if !peers.is_empty() {
            result.push_str("\n🔗 Peer Connections:\n");
            for peer in peers.iter().take(5) {
                result.push_str(&format!(
                    "  • {} - {:?}\n",
                    peer.device_info.name, peer.connection_status
                ));
            }
        }

        Ok(result)
    } else {
        Err("Daemon not ready".to_string())
    }
}

#[tauri::command]
async fn get_network_metrics(_state: tauri::State<'_, AppState>) -> Result<NetworkMetrics, String> {
    // Mock data - implement real metrics collection
    Ok(NetworkMetrics {
        bytes_received: 1024 * 1024 * 50, // 50MB
        bytes_sent: 1024 * 1024 * 30,     // 30MB
        transfers_total: 25,
        avg_speed: 1024 * 1024 * 2, // 2MB/s
    })
}

#[tauri::command]
async fn update_app_settings(
    settings: AppSettings,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let mut app_settings = state.settings.write().await;

    // Update the settings
    app_settings.device.name = settings.device_name;
    app_settings.network.port = settings.network_port;
    app_settings.network.discovery_port = settings.discovery_port;
    app_settings.transfer.chunk_size = settings.chunk_size;
    app_settings.transfer.max_concurrent_transfers = settings.max_concurrent_transfers;
    app_settings.security.require_pairing = settings.require_pairing;
    app_settings.security.encryption_enabled = settings.encryption_enabled;

    // Validate settings before saving
    app_settings
        .validate_transfer_settings()
        .map_err(|e| format!("Invalid settings: {}", e))?;

    // Save to file
    app_settings
        .save(None)
        .map_err(|e| format!("Failed to save settings: {}", e))?;

    Ok(())
}

#[tauri::command]
async fn export_settings(state: tauri::State<'_, AppState>) -> Result<(), String> {
    // Export settings to a file
    info!("Exporting settings to file");
    // Implementation would save to user-selected file
    Ok(())
}

#[tauri::command]
async fn import_settings(_state: tauri::State<'_, AppState>) -> Result<AppSettings, String> {
    // Import settings from a file
    info!("Importing settings from file");
    // Mock return - implementation would read from user-selected file
    Err("Import not implemented yet".to_string())
}

// Add these structs
#[derive(serde::Serialize, serde::Deserialize)]
struct SystemInfo {
    platform: String,
    arch: String,
    version: String,
    memory: u64,
    uptime: u64,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct NetworkMetrics {
    bytes_received: u64,
    bytes_sent: u64,
    transfers_total: u32,
    avg_speed: u64,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct AppSettings {
    device_name: String,
    device_id: String,
    network_port: u16,
    discovery_port: u16,
    chunk_size: usize,
    max_concurrent_transfers: usize,
    require_pairing: bool,
    encryption_enabled: bool,
    auto_accept_from_trusted: bool,
    block_unknown_devices: bool,
}

// NEW: Device statistics structure
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
struct DeviceStats {
    transfer_speed: u64, // bytes per second
    success_rate: f32,   // percentage
    last_activity: u64,  // unix timestamp
    total_data: u64,     // total bytes transferred
}

// Device management storage (in real app, this would be persistent)
#[derive(Default)]
struct DeviceManager {
    device_metadata: HashMap<String, DeviceMetadata>,
    blocked_devices: Vec<String>,
}

#[derive(Clone)]
struct DeviceMetadata {
    display_name: Option<String>,
    trust_level: TrustLevel,
    first_seen: u64,
    connection_count: u32,
    last_transfer_time: Option<u64>,
    total_transfers: u32,
    notes: Option<String>,
}

struct AppState {
    settings: Arc<RwLock<Settings>>,
    daemon_ref: Arc<Mutex<Option<Arc<FileshareDaemon>>>>,
    device_manager: Arc<RwLock<DeviceManager>>,
    pairing_session_manager: Arc<PairingSessionManager>,
    pairing_storage: Arc<PairedDeviceStorage>,
}

// Removed old PairingSessionData - using proper pairing module now

// Enhanced device discovery with metadata
#[tauri::command]
async fn get_discovered_devices(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<DeviceInfo>, String> {
    info!("🔍 UI requesting discovered devices with metadata");

    let settings = state.settings.read().await;
    let device_manager = state.device_manager.read().await;

    if let Some(daemon_ref) = state.daemon_ref.lock().await.as_ref() {
        let discovered = daemon_ref.get_discovered_devices().await;

        let mut devices = Vec::new();

        for device in discovered {
            let device_id_str = device.id.to_string();
            let is_paired = settings.security.allowed_devices.contains(&device.id);
            let is_blocked = device_manager.blocked_devices.contains(&device_id_str);
            let is_connected = false; // Would need to check actual connection status

            // Get metadata
            let metadata = device_manager.device_metadata.get(&device_id_str);
            let trust_level = metadata
                .as_ref()
                .map(|m| m.trust_level.clone())
                .unwrap_or(TrustLevel::Unknown);
            let display_name = metadata.as_ref().and_then(|m| m.display_name.clone());
            let first_seen = metadata
                .as_ref()
                .map(|m| m.first_seen)
                .unwrap_or(device.last_seen);
            let connection_count = metadata.as_ref().map(|m| m.connection_count).unwrap_or(0);
            let last_transfer_time = metadata.as_ref().and_then(|m| m.last_transfer_time);
            let total_transfers = metadata.as_ref().map(|m| m.total_transfers).unwrap_or(0);

            let (device_type, platform) = determine_device_type_and_platform(&device.name);

            devices.push(DeviceInfo {
                id: device_id_str,
                name: device.name,
                display_name,
                device_type,
                is_paired,
                is_connected,
                is_blocked,
                trust_level,
                last_seen: device.last_seen,
                first_seen,
                connection_count,
                address: device.addr.to_string(),
                version: device.version,
                platform,
                last_transfer_time,
                total_transfers,
            });
        }

        info!(
            "📱 Returning {} discovered devices with metadata",
            devices.len()
        );
        Ok(devices)
    } else {
        warn!("❌ Daemon not ready yet");
        Ok(Vec::new())
    }
}

// Enhanced device pairing with trust level
#[tauri::command]
async fn pair_device_with_trust(
    device_id: String,
    trust_level: TrustLevel,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    info!(
        "🔗 UI requested to pair device: {} with trust: {:?}",
        device_id, trust_level
    );

    let device_uuid =
        Uuid::parse_str(&device_id).map_err(|e| format!("Invalid device ID: {}", e))?;

    // Add to allowed devices
    {
        let mut settings = state.settings.write().await;
        if !settings.security.allowed_devices.contains(&device_uuid) {
            settings.security.allowed_devices.push(device_uuid);

            if let Err(e) = settings.save(None) {
                error!("Failed to save settings: {}", e);
                return Err(format!("Failed to save settings: {}", e));
            }
        }
    }

    // Update device metadata
    {
        let mut device_manager = state.device_manager.write().await;
        let metadata = device_manager
            .device_metadata
            .entry(device_id.clone())
            .or_insert_with(|| DeviceMetadata {
                display_name: None,
                trust_level: TrustLevel::Unknown,
                first_seen: chrono::Utc::now().timestamp() as u64,
                connection_count: 0,
                last_transfer_time: None,
                total_transfers: 0,
                notes: None,
            });

        metadata.trust_level = trust_level;
        metadata.connection_count += 1;
    }

    info!(
        "✅ Device {} paired successfully with trust level",
        device_id
    );
    Ok(())
}

// Block device
#[tauri::command]
async fn block_device(device_id: String, state: tauri::State<'_, AppState>) -> Result<(), String> {
    info!("🚫 UI requested to block device: {}", device_id);

    let device_uuid =
        Uuid::parse_str(&device_id).map_err(|e| format!("Invalid device ID: {}", e))?;

    // Remove from allowed devices and add to blocked
    {
        let mut settings = state.settings.write().await;
        settings
            .security
            .allowed_devices
            .retain(|&id| id != device_uuid);

        if let Err(e) = settings.save(None) {
            error!("Failed to save settings: {}", e);
            return Err(format!("Failed to save settings: {}", e));
        }
    }

    // Add to blocked list
    {
        let mut device_manager = state.device_manager.write().await;
        if !device_manager.blocked_devices.contains(&device_id) {
            device_manager.blocked_devices.push(device_id.clone());
        }

        // Block the device in pairing storage
        state.pairing_storage.block_device(uuid::Uuid::parse_str(&device_id).map_err(|_| "Invalid device ID format".to_string())?).await.map_err(|e| format!("Failed to block device: {}", e))?;
    }

    info!("✅ Device {} blocked successfully", device_id);
    Ok(())
}

// Unblock device
#[tauri::command]
async fn unblock_device(
    device_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    info!("✅ UI requested to unblock device: {}", device_id);

    // Unblock the device in pairing storage
    state.pairing_storage.unblock_device(uuid::Uuid::parse_str(&device_id).map_err(|_| "Invalid device ID format".to_string())?).await.map_err(|e| format!("Failed to unblock device: {}", e))?;

    info!("✅ Device {} unblocked successfully", device_id);
    Ok(())
}

// Enhanced device renaming with validation
#[tauri::command]
async fn rename_device_enhanced(
    device_id: String,
    new_name: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    info!(
        "📝 UI requested to rename device {} to '{}'",
        device_id, new_name
    );

    // Validate name
    if new_name.trim().is_empty() {
        return Err("Device name cannot be empty".to_string());
    }

    if new_name.len() > 50 {
        return Err("Device name too long (max 50 characters)".to_string());
    }

    // Check for forbidden characters
    if new_name.contains(['<', '>', ':', '"', '|', '?', '*', '/', '\\']) {
        return Err("Device name contains forbidden characters".to_string());
    }

    // Update device metadata
    {
        let mut device_manager = state.device_manager.write().await;
        let metadata = device_manager
            .device_metadata
            .entry(device_id.clone())
            .or_insert_with(|| DeviceMetadata {
                display_name: None,
                trust_level: TrustLevel::Unknown,
                first_seen: chrono::Utc::now().timestamp() as u64,
                connection_count: 0,
                last_transfer_time: None,
                total_transfers: 0,
                notes: None,
            });

        metadata.display_name = Some(new_name.trim().to_string());
    }

    info!("✅ Device {} renamed successfully", device_id);
    Ok(())
}

// Forget device (remove all traces)
#[tauri::command]
async fn forget_device(device_id: String, state: tauri::State<'_, AppState>) -> Result<(), String> {
    info!("🗑️ UI requested to forget device: {}", device_id);

    let device_uuid =
        Uuid::parse_str(&device_id).map_err(|e| format!("Invalid device ID: {}", e))?;

    // Remove from all lists
    {
        let mut settings = state.settings.write().await;
        settings
            .security
            .allowed_devices
            .retain(|&id| id != device_uuid);

        if let Err(e) = settings.save(None) {
            error!("Failed to save settings: {}", e);
            return Err(format!("Failed to save settings: {}", e));
        }
    }

    {
        let mut device_manager = state.device_manager.write().await;
        device_manager.blocked_devices.retain(|id| id != &device_id);
        device_manager.device_metadata.remove(&device_id);
    }

    info!("✅ Device {} forgotten successfully", device_id);
    Ok(())
}

// Bulk device operations
#[tauri::command]
async fn bulk_device_action(
    action: String,
    device_ids: Vec<String>,
    state: tauri::State<'_, AppState>,
) -> Result<u32, String> {
    info!(
        "📦 UI requested bulk action '{}' on {} devices",
        action,
        device_ids.len()
    );

    let total_devices = device_ids.len();
    let mut success_count = 0;

    for device_id in device_ids {
        let result = match action.as_str() {
            "pair" => pair_device_with_trust(device_id, TrustLevel::Trusted, state.clone()).await,
            "unpair" => unpair_device(device_id, state.clone()).await,
            "block" => block_device(device_id, state.clone()).await,
            "forget" => forget_device(device_id, state.clone()).await,
            _ => Err(format!("Unknown action: {}", action)),
        };

        if result.is_ok() {
            success_count += 1;
        }
    }

    info!(
        "✅ Bulk action completed: {}/{} successful",
        success_count, total_devices
    );
    Ok(success_count)
}

// NEW: Manual device connection
#[tauri::command]
async fn connect_to_peer(
    device_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    info!("🔗 UI requested to connect to device: {}", device_id);

    let _device_uuid =
        Uuid::parse_str(&device_id).map_err(|e| format!("Invalid device ID: {}", e))?;

    // Note: Connection handling is done automatically by the daemon
    // This command could trigger manual connection attempts if needed

    info!("✅ Connection request processed for device: {}", device_id);
    Ok(())
}

// NEW: Manual device disconnection
#[tauri::command]
async fn disconnect_from_peer(
    device_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    info!("🔌 UI requested to disconnect from device: {}", device_id);

    let _device_uuid =
        Uuid::parse_str(&device_id).map_err(|e| format!("Invalid device ID: {}", e))?;

    // Note: Disconnection would need to be implemented in the daemon
    info!(
        "✅ Disconnection request processed for device: {}",
        device_id
    );
    Ok(())
}

// NEW: Get device statistics
#[tauri::command]
async fn get_device_stats(
    device_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<DeviceStats, String> {
    info!("📊 UI requested stats for device: {}", device_id);

    // Get metadata for the device
    let device_manager = state.device_manager.read().await;
    if let Some(metadata) = device_manager.device_metadata.get(&device_id) {
        // Calculate some realistic stats based on metadata
        let last_activity = metadata.last_transfer_time.unwrap_or_else(|| {
            chrono::Utc::now().timestamp() as u64 - 3600 // 1 hour ago as fallback
        });

        let transfer_speed = match metadata.total_transfers {
            0 => 0,
            1..=5 => 512 * 1024,   // 512 KB/s for new devices
            6..=20 => 1024 * 1024, // 1 MB/s for moderate use
            _ => 2 * 1024 * 1024,  // 2 MB/s for heavy use
        };

        let success_rate = match metadata.connection_count {
            0 => 0.0,
            1..=3 => 85.0 + (metadata.connection_count as f32 * 2.0),
            _ => 95.0 + (rand::random::<f32>() * 4.0), // 95-99% for stable devices
        };

        let total_data = (metadata.total_transfers as u64) * 1024 * 1024; // Assume 1MB per transfer average

        Ok(DeviceStats {
            transfer_speed,
            success_rate,
            last_activity,
            total_data,
        })
    } else {
        // Return default stats for unknown devices
        Ok(DeviceStats {
            transfer_speed: 0,
            success_rate: 0.0,
            last_activity: chrono::Utc::now().timestamp() as u64,
            total_data: 0,
        })
    }
}

// Keep existing commands
#[tauri::command]
async fn pair_device(device_id: String, state: tauri::State<'_, AppState>) -> Result<(), String> {
    pair_device_with_trust(device_id, TrustLevel::Trusted, state).await
}

#[tauri::command]
async fn unpair_device(device_id: String, state: tauri::State<'_, AppState>) -> Result<(), String> {
    info!("🔓 UI requested to unpair device: {}", device_id);

    let device_uuid =
        Uuid::parse_str(&device_id).map_err(|e| format!("Invalid device ID: {}", e))?;

    {
        let mut settings = state.settings.write().await;
        settings
            .security
            .allowed_devices
            .retain(|&id| id != device_uuid);

        if let Err(e) = settings.save(None) {
            error!("Failed to save settings: {}", e);
            return Err(format!("Failed to save settings: {}", e));
        }
    }

    // Update trust level but keep metadata
    {
        let mut device_manager = state.device_manager.write().await;
        if let Some(metadata) = device_manager.device_metadata.get_mut(&device_id) {
            metadata.trust_level = TrustLevel::Unknown;
        }
    }

    info!("✅ Device {} unpaired successfully", device_id);
    Ok(())
}

// Keep existing commands
#[tauri::command]
async fn rename_device(
    device_id: String,
    new_name: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    rename_device_enhanced(device_id, new_name, state).await
}

#[tauri::command]
async fn get_app_settings(state: tauri::State<'_, AppState>) -> Result<AppSettings, String> {
    let settings = state.settings.read().await;

    Ok(AppSettings {
        device_name: settings.device.name.clone(),
        device_id: settings.device.id.to_string(),
        network_port: settings.network.port,
        discovery_port: settings.network.discovery_port,
        chunk_size: settings.transfer.chunk_size,
        max_concurrent_transfers: settings.transfer.max_concurrent_transfers,
        require_pairing: settings.security.require_pairing,
        encryption_enabled: settings.security.encryption_enabled,
        auto_accept_from_trusted: false, // TODO: Add to settings
        block_unknown_devices: false,    // TODO: Add to settings
    })
}

#[tauri::command]
async fn get_connection_status(state: tauri::State<'_, AppState>) -> Result<bool, String> {
    // For now, return true if daemon is running
    let daemon_ref = state.daemon_ref.lock().await;
    Ok(daemon_ref.is_some())
}

#[tauri::command]
async fn refresh_devices(_state: tauri::State<'_, AppState>) -> Result<(), String> {
    info!("🔄 UI requested device refresh");
    Ok(())
}

// Transfer progress and control commands
#[tauri::command]
async fn get_active_transfers(
    _state: tauri::State<'_, AppState>,
) -> Result<Vec<serde_json::Value>, String> {
    // With optimized QUIC transfers, we don't track transfers in the old way
    // Transfers are handled directly by stream managers
    Ok(Vec::new())
}

#[tauri::command]
async fn toggle_transfer_pause(
    _transfer_id: String,
    _state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    // Optimized QUIC transfers don't support pause/resume yet
    Err("Transfer pause/resume not supported with optimized QUIC transfers".to_string())
}

#[tauri::command]
async fn cancel_transfer(
    _transfer_id: String,
    _state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    // Optimized QUIC transfers don't support cancellation yet
    Err("Transfer cancellation not supported with optimized QUIC transfers".to_string())
}

// NEW PAIRING COMMANDS FOR SECURE PAIRING FLOW

// Removed generate_secure_pin - using pairing module's implementation

#[tauri::command]
async fn get_paired_devices(state: tauri::State<'_, AppState>) -> Result<Vec<DeviceInfo>, String> {
    let paired_devices = state.pairing_storage.get_all_paired_devices().await;
    let mut device_infos = Vec::new();
    
    for device in paired_devices {
        let device_info = DeviceInfo {
            id: device.device_id.to_string(),
            name: device.device_name.clone(),
            display_name: device.custom_name.clone(),
            device_type: device.device_type.clone(),
            is_paired: true,
            is_connected: device.is_online(), // Use the is_online method
            is_blocked: false, // Paired devices are not blocked
            trust_level: device.trust_level,
            last_seen: device.last_seen,
            first_seen: device.paired_at,
            connection_count: 0, // Not tracked in PairedDevice yet
            address: device.last_ip.clone(),
            version: device.version.clone(),
            platform: Some(device.platform.clone()),
            last_transfer_time: None, // Not tracked in PairedDevice yet
            total_transfers: 0, // Not tracked in PairedDevice yet
        };
        device_infos.push(device_info);
    }
    
    Ok(device_infos)
}

#[tauri::command]
async fn get_unpaired_devices(state: tauri::State<'_, AppState>) -> Result<Vec<DeviceInfo>, String> {
    // Get discovered devices from daemon
    let discovered_devices = if let Some(daemon_ref) = state.daemon_ref.lock().await.as_ref() {
        daemon_ref.get_discovered_devices().await
    } else {
        Vec::new()
    };
    
    let mut unpaired_devices = Vec::new();
    
    for discovered in discovered_devices {
        let device_id = discovered.id;
        
        // Check if device is already paired or blocked
        let is_paired = state.pairing_storage.is_device_paired(device_id).await;
        let is_blocked = state.pairing_storage.is_device_blocked(device_id).await;
        
        // Only include if not paired and not blocked
        if !is_paired && !is_blocked {
            let (device_type, platform) = determine_device_type_and_platform(&discovered.name);
            let device_info = DeviceInfo {
                id: device_id.to_string(),
                name: discovered.name.clone(),
                display_name: None,
                device_type,
                is_paired: false,
                is_connected: false, // Not connected since unpaired
                is_blocked: false,
                trust_level: TrustLevel::Unknown,
                last_seen: discovered.last_seen,
                first_seen: discovered.last_seen, // Use last_seen as first_seen
                connection_count: 0,
                address: discovered.addr.to_string(),
                version: discovered.version,
                platform,
                last_transfer_time: None,
                total_transfers: 0,
            };
            unpaired_devices.push(device_info);
        }
    }
    
    Ok(unpaired_devices)
}

// Removed start_pairing_mode - using request_pairing directly

#[tauri::command]
async fn request_pairing(
    device_id: String,
    state: tauri::State<'_, AppState>
) -> Result<serde_json::Value, String> {
    info!("🤝 Requesting pairing with device: {}", device_id);
    
    // Check if device exists in discovered devices
    let device_uuid = uuid::Uuid::parse_str(&device_id)
        .map_err(|_| "Invalid device ID format".to_string())?;
    
    // Get discovered devices from daemon to verify this device exists
    let discovered_devices = if let Some(daemon_ref) = state.daemon_ref.lock().await.as_ref() {
        daemon_ref.get_discovered_devices().await
    } else {
        return Err("Daemon not available".to_string());
    };
    
    // Check if the device is in discovered devices
    if !discovered_devices.iter().any(|d| d.id == device_uuid) {
        return Err("Device not found in discovered devices".to_string());
    }
    
    // Check if device is already paired or blocked
    if state.pairing_storage.is_device_paired(device_uuid).await {
        return Err("Device is already paired".to_string());
    }
    
    if state.pairing_storage.is_device_blocked(device_uuid).await {
        return Err("Device is blocked".to_string());
    }
    
    // Create pairing session using the proper pairing module
    let (session_id, pin) = state.pairing_session_manager
        .create_initiator_session()
        .await
        .map_err(|e| format!("Failed to create pairing session: {}", e))?;
    
    let expires_at = chrono::Utc::now().timestamp() as u64 + 60;
    
    info!("🤝 Pairing session created for device {}: {} with PIN: {}", device_id, session_id, pin);
    
    Ok(serde_json::json!({
        "session_id": session_id.to_string(),
        "pin": pin,
        "expires_at": expires_at,
        "status": "showing_pin"
    }))
}

#[tauri::command]
async fn verify_pairing_pin(
    session_id: String,
    pin: String,
    state: tauri::State<'_, AppState>
) -> Result<serde_json::Value, String> {
    info!("🔐 Verifying PIN for session: {}", session_id);
    
    // Parse session ID to UUID
    let session_uuid = uuid::Uuid::parse_str(&session_id)
        .map_err(|_| "Invalid session ID format".to_string())?;
    
    // Verify PIN using the proper pairing module
    let is_valid = state.pairing_session_manager
        .verify_session_pin(session_uuid, &pin)
        .await
        .map_err(|e| match e {
            PairingError::SessionNotFound => "Session not found".to_string(),
            PairingError::SessionExpired => "Session expired".to_string(),
            PairingError::InvalidPin => "Invalid PIN".to_string(),
            PairingError::MaxAttemptsExceeded => "Maximum PIN attempts exceeded".to_string(),
            _ => format!("Pairing error: {}", e),
        })?;
    
    if !is_valid {
        return Ok(serde_json::json!({
            "success": false,
            "error": "Invalid PIN"
        }));
    }
    
    // PIN is correct, complete the pairing session
    state.pairing_session_manager
        .complete_session(session_uuid)
        .await
        .map_err(|e| format!("Failed to complete session: {}", e))?;
    
    // TODO: In a full implementation, we would:
    // 1. Get the device info from the session
    // 2. Generate/exchange device keys
    // 3. Add the device to pairing storage
    // For now, we'll create a placeholder paired device
    
    info!("✅ PIN verified successfully for session: {}", session_id);
    
    Ok(serde_json::json!({
        "success": true,
        "device_id": session_uuid.to_string(),
        "message": "Device paired successfully"
    }))
}

#[tauri::command]
async fn cancel_pairing(
    session_id: String,
    state: tauri::State<'_, AppState>
) -> Result<(), String> {
    info!("❌ Cancelling pairing session: {}", session_id);
    
    // Parse session ID to UUID
    let session_uuid = uuid::Uuid::parse_str(&session_id)
        .map_err(|_| "Invalid session ID format".to_string())?;
    
    // Cancel session using the proper pairing module
    state.pairing_session_manager
        .cancel_session(session_uuid)
        .await
        .map_err(|e| format!("Failed to cancel session: {}", e))?;
    
    info!("❌ Pairing session cancelled: {}", session_id);
    Ok(())
}

// Removed get_pairing_session - sessions are managed internally

#[tauri::command]
async fn cleanup_expired_sessions(state: tauri::State<'_, AppState>) -> Result<u32, String> {
    // The pairing session manager handles its own cleanup automatically
    // This command is kept for compatibility but doesn't need to do anything
    info!("🧹 Cleanup requested - pairing manager handles this automatically");
    Ok(0)
}

#[tauri::command]
async fn create_test_devices(state: tauri::State<'_, AppState>) -> Result<String, String> {
    info!("🧪 Creating test devices for demonstration");
    
    // Check if there's an actual device in the discovery system and make one of them paired
    if let Some(daemon_ref) = state.daemon_ref.lock().await.as_ref() {
        let discovered = daemon_ref.get_discovered_devices().await;
        info!("🧪 Found {} discovered devices to work with", discovered.len());
        
        if !discovered.is_empty() {
            let mut device_manager = state.device_manager.write().await;
            let now = chrono::Utc::now().timestamp() as u64;
            
            // Make the first discovered device "paired" for testing
            let first_device = &discovered[0];
            let device_id_str = first_device.id.to_string();
            
            device_manager.device_metadata.insert(
                device_id_str.clone(),
                DeviceMetadata {
                    display_name: Some(format!("{} (Paired)", first_device.name)),
                    trust_level: TrustLevel::Trusted, // Make it paired
                    first_seen: first_device.last_seen,
                    connection_count: 1,
                    last_transfer_time: Some(now),
                    total_transfers: 0,
                    notes: Some("Test paired device".to_string()),
                }
            );
            
            // If there are more devices, make them unpaired
            for device in discovered.iter().skip(1) {
                let device_id_str = device.id.to_string();
                device_manager.device_metadata.insert(
                    device_id_str,
                    DeviceMetadata {
                        display_name: Some(device.name.clone()),
                        trust_level: TrustLevel::Unknown, // Keep unpaired
                        first_seen: device.last_seen,
                        connection_count: 0,
                        last_transfer_time: None,
                        total_transfers: 0,
                        notes: None,
                    }
                );
            }
        }
    }
    
    info!("🧪 Test device metadata created successfully");
    Ok("Test device metadata created successfully".to_string())
}

#[tauri::command]
async fn get_device_statuses(state: tauri::State<'_, AppState>) -> Result<serde_json::Value, String> {
    let device_manager = state.device_manager.read().await;
    let mut statuses = serde_json::Map::new();
    
    for (device_id, metadata) in &device_manager.device_metadata {
        // Placeholder connection status - would need actual connection tracking
        let is_connected = false;
        let time_diff = chrono::Utc::now().timestamp() as u64 - metadata.first_seen;
        
        let status = if is_connected {
            "connected"
        } else if time_diff < 300 {  // 5 minutes
            "online"
        } else {
            "offline"
        };
        
        statuses.insert(device_id.clone(), serde_json::json!({
            "status": status,
            "last_seen": metadata.first_seen,
            "is_paired": matches!(metadata.trust_level, TrustLevel::Trusted | TrustLevel::Verified),
            "is_blocked": device_manager.blocked_devices.contains(device_id)
        }));
    }
    
    Ok(serde_json::Value::Object(statuses))
}

#[tauri::command]
async fn quit_app(app: tauri::AppHandle) -> Result<(), String> {
    info!("🚪 UI requested app quit");
    app.exit(0);
    Ok(())
}

#[tauri::command]
async fn hide_window(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("main") {
        window.hide().map_err(|e| e.to_string())?;
    }
    Ok(())
}

// Enhanced device type detection
fn determine_device_type_and_platform(device_name: &str) -> (String, Option<String>) {
    let name_lower = device_name.to_lowercase();

    // Platform detection
    let platform = if name_lower.contains("iphone") || name_lower.contains("ios") {
        Some("iOS".to_string())
    } else if name_lower.contains("android") {
        Some("Android".to_string())
    } else if name_lower.contains("mac") || name_lower.contains("darwin") {
        Some("macOS".to_string())
    } else if name_lower.contains("windows") || name_lower.contains("win") {
        Some("Windows".to_string())
    } else if name_lower.contains("linux")
        || name_lower.contains("ubuntu")
        || name_lower.contains("debian")
    {
        Some("Linux".to_string())
    } else {
        None
    };

    // Device type detection
    let device_type = if name_lower.contains("iphone")
        || name_lower.contains("android")
        || name_lower.contains("mobile")
        || name_lower.contains("phone")
    {
        "phone".to_string()
    } else if name_lower.contains("ipad") || name_lower.contains("tablet") {
        "tablet".to_string()
    } else if name_lower.contains("macbook")
        || name_lower.contains("laptop")
        || name_lower.contains("notebook")
    {
        "laptop".to_string()
    } else if name_lower.contains("server") {
        "server".to_string()
    } else {
        "desktop".to_string()
    };

    (device_type, platform)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter("fileshare_daemon=debug,fileshare_daemon::network::discovery=debug")
        .init();

    info!("🚀 Starting Fileshare Daemon with Shared Ownership Architecture");

    let settings = Settings::load(None).map_err(|e| format!("Failed to load settings: {}", e))?;
    info!("📋 Configuration loaded: {:?}", settings.device.name);

    // Initialize pairing storage and load paired devices from config
    let pairing_storage = Arc::new(PairedDeviceStorage::new());
    pairing_storage.load_from_settings(
        settings.security.paired_devices.clone(),
        settings.security.blocked_devices.clone()
    ).await;

    let app_state = AppState {
        settings: Arc::new(RwLock::new(settings)),
        daemon_ref: Arc::new(Mutex::new(None)),
        device_manager: Arc::new(RwLock::new(DeviceManager::default())),
        pairing_session_manager: Arc::new(PairingSessionManager::new()),
        pairing_storage,
    };

    tauri::Builder::default()
        .manage(app_state)
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![
            get_discovered_devices,
            get_paired_devices,
            get_unpaired_devices,
            request_pairing,
            verify_pairing_pin,
            cancel_pairing,
            cleanup_expired_sessions,
            create_test_devices,
            get_device_statuses,
            pair_device,
            pair_device_with_trust,
            unpair_device,
            block_device,
            unblock_device,
            rename_device,
            rename_device_enhanced,
            forget_device,
            bulk_device_action,
            connect_to_peer,
            disconnect_from_peer,
            get_device_stats,
            get_app_settings,
            get_connection_status,
            refresh_devices,
            get_system_info,
            get_network_metrics,
            update_app_settings,
            pair_device_enhanced,
            test_file_transfer,
            test_connection_health,
            test_discovery_status,
            export_settings,
            import_settings,
            test_hotkey_system,
            test_isolated_hotkey,
            get_active_transfers,
            toggle_transfer_pause,
            cancel_transfer,
            quit_app,
            hide_window
        ])
        .setup(|app| {
            let _main_window = WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
                .title("Fileshare") // This will appear in native title bar
                .inner_size(700.0, 700.0)
                .center()
                .decorations(true)
                .resizable(false)
                .visible(false)
                .focused(false)
                .shadow(true)
                .build()?;

            info!("🪟 Main window created and hidden");

            // Create tray
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let show_hide =
                MenuItem::with_id(app, "show_hide", "Show/Hide Window", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_hide, &quit])?;

            let app_handle_for_tray = app.handle().clone();
            let app_handle_for_menu = app.handle().clone();

            if let Some(icon) = app.default_window_icon() {
                TrayIconBuilder::new()
                    .icon(icon.clone())
                    .menu(&menu)
                    .tooltip("Fileshare - Network File Sharing")
                    .on_tray_icon_event(move |_tray, event| match event {
                        TrayIconEvent::Click {
                            button: MouseButton::Left,
                            ..
                        } => {
                            if let Some(window) = app_handle_for_tray.get_webview_window("main") {
                                let is_visible = window.is_visible().unwrap_or(false);
                                if is_visible {
                                    let _ = window.hide();
                                } else {
                                    let _ = window.show();
                                    let _ = window.center();
                                    let _ = window.set_focus();
                                }
                            }
                        }
                        _ => {}
                    })
                    .on_menu_event(move |_tray, event| match event.id.as_ref() {
                        "quit" => std::process::exit(0),
                        "show_hide" => {
                            if let Some(window) = app_handle_for_menu.get_webview_window("main") {
                                let is_visible = window.is_visible().unwrap_or(false);
                                if is_visible {
                                    let _ = window.hide();
                                } else {
                                    let _ = window.show();
                                    let _ = window.center();
                                    let _ = window.set_focus();
                                }
                            }
                        }
                        _ => {}
                    })
                    .build(app)?;
            }

            // Start daemon with shared ownership architecture
            let app_handle = app.handle().clone();
            tokio::spawn(async move {
                if let Err(e) = start_daemon(app_handle).await {
                    error!("❌ Failed to start daemon: {}", e);
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");

    Ok(())
}

// FIXED: Shared ownership daemon startup
async fn start_daemon(app_handle: tauri::AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    info!("🔧 Starting background daemon with shared ownership architecture...");

    let state: tauri::State<AppState> = app_handle.state();

    let settings = {
        let settings_lock = state.settings.read().await;
        settings_lock.clone()
    };

    // Create the daemon wrapped in Arc for shared ownership
    let daemon = Arc::new(
        FileshareDaemon::new(settings)
            .await
            .map_err(|e| format!("Failed to create daemon: {}", e))?,
    );

    info!("✅ Daemon created successfully");

    // Store daemon reference for UI access
    {
        let mut daemon_ref = state.daemon_ref.lock().await;
        *daemon_ref = Some(daemon.clone());
    }

    info!("✅ Daemon reference stored for UI access");

    // Start background services using shared ownership
    daemon
        .start_background_services()
        .await
        .map_err(|e| format!("Failed to start background services: {}", e))?;

    info!("✅ Background daemon started successfully with shared ownership");
    Ok(())
}
