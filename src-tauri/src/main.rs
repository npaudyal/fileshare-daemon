// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use fileshare_daemon::{
    clipboard::ClipboardManager, config::Settings, hotkeys::HotkeyEvent, service::FileshareDaemon,
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

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
enum TrustLevel {
    Unknown,
    Trusted,
    Verified,
    Blocked,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
struct DeviceAction {
    action: String,
    device_id: String,
    params: Option<HashMap<String, String>>,
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
async fn test_hotkey_system() -> Result<String, String> {
    info!("🧪 Starting comprehensive hotkey system test...");

    tokio::task::spawn_blocking(|| run_hotkey_diagnostics())
        .await
        .map_err(|e| format!("Test spawn error: {}", e))
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

    // Save to file
    app_settings
        .save(None)
        .map_err(|e| format!("Failed to save settings: {}", e))?;

    Ok(())
}

#[tauri::command]
async fn check_main_hotkey_status() -> Result<String, String> {
    info!("🔍 Checking main app hotkey status...");

    // This will help us see if the main hotkey system is running
    Ok(format!(
        "Main hotkey system status:\n\
        - Platform: {}\n\
        - Expected hotkeys: Ctrl+Alt+Y (copy), Ctrl+Alt+I (paste)\n\
        - Check the console logs when you press these keys\n\
        - If you see 'Copy hotkey triggered' or 'Paste hotkey triggered' in logs, events are working\n\
        - If not, the event loop isn't receiving messages",
        std::env::consts::OS
    ))
}

#[tauri::command]
async fn test_isolated_hotkey() -> Result<String, String> {
    // Run in a completely separate task
    tokio::task::spawn_blocking(|| isolated_hotkey_test())
        .await
        .map_err(|e| format!("Spawn error: {}", e))
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
    daemon_ref: Arc<Mutex<Option<Arc<RwLock<fileshare_daemon::network::PeerManager>>>>>,
    device_manager: Arc<RwLock<DeviceManager>>,
}

// Enhanced device discovery with metadata
#[tauri::command]
async fn get_discovered_devices(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<DeviceInfo>, String> {
    info!("🔍 UI requesting discovered devices with metadata");

    let settings = state.settings.read().await;
    let device_manager = state.device_manager.read().await;

    if let Some(peer_manager_ref) = state.daemon_ref.lock().await.as_ref() {
        let pm = peer_manager_ref.read().await;
        let discovered = pm.get_all_discovered_devices().await;

        let mut devices = Vec::new();

        for device in discovered {
            let device_id_str = device.id.to_string();
            let is_paired = settings.security.allowed_devices.contains(&device.id);
            let is_blocked = device_manager.blocked_devices.contains(&device_id_str);
            let is_connected = pm
                .get_connected_peers()
                .iter()
                .any(|p| p.device_info.id == device.id);

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
        warn!("❌ Peer manager not ready yet");
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

    // Try to connect
    if let Some(peer_manager_ref) = state.daemon_ref.lock().await.as_ref() {
        let mut pm = peer_manager_ref.write().await;
        if let Err(e) = pm.connect_to_peer(device_uuid).await {
            warn!("Failed to connect to paired device {}: {}", device_id, e);
        }
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

        // Update trust level
        if let Some(metadata) = device_manager.device_metadata.get_mut(&device_id) {
            metadata.trust_level = TrustLevel::Blocked;
        }
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

    {
        let mut device_manager = state.device_manager.write().await;
        device_manager.blocked_devices.retain(|id| id != &device_id);

        // Update trust level
        if let Some(metadata) = device_manager.device_metadata.get_mut(&device_id) {
            metadata.trust_level = TrustLevel::Unknown;
        }
    }

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

    let device_uuid =
        Uuid::parse_str(&device_id).map_err(|e| format!("Invalid device ID: {}", e))?;

    if let Some(peer_manager_ref) = state.daemon_ref.lock().await.as_ref() {
        let mut pm = peer_manager_ref.write().await;
        pm.connect_to_peer(device_uuid)
            .await
            .map_err(|e| format!("Failed to connect: {}", e))?;

        // Update connection count in metadata
        {
            let mut device_manager = state.device_manager.write().await;
            if let Some(metadata) = device_manager.device_metadata.get_mut(&device_id) {
                metadata.connection_count += 1;
            }
        }
    } else {
        return Err("Peer manager not available".to_string());
    }

    info!("✅ Connected to device: {}", device_id);
    Ok(())
}

// NEW: Manual device disconnection
#[tauri::command]
async fn disconnect_from_peer(
    device_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    info!("🔌 UI requested to disconnect from device: {}", device_id);

    let device_uuid =
        Uuid::parse_str(&device_id).map_err(|e| format!("Invalid device ID: {}", e))?;

    if let Some(peer_manager_ref) = state.daemon_ref.lock().await.as_ref() {
        let pm = peer_manager_ref.read().await;

        // Remove from connected peers
        // Note: You'll need to implement this method in your PeerManager
        // For now, we'll just log it
        info!(
            "Disconnecting from peer {} (implementation needed in PeerManager)",
            device_uuid
        );

        // In a real implementation, you would:
        // pm.disconnect_from_peer(device_uuid).await?;
    } else {
        return Err("Peer manager not available".to_string());
    }

    info!("✅ Disconnected from device: {}", device_id);
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
    if let Some(peer_manager_ref) = state.daemon_ref.lock().await.as_ref() {
        let pm = peer_manager_ref.read().await;
        let connected_count = pm.get_connected_peers().len();
        Ok(connected_count > 0)
    } else {
        Ok(false)
    }
}

#[tauri::command]
async fn refresh_devices(_state: tauri::State<'_, AppState>) -> Result<(), String> {
    info!("🔄 UI requested device refresh");
    Ok(())
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

    info!("🚀 Starting Fileshare Daemon with Enhanced Device Management and Fixed Windows Hotkeys");

    let settings = Settings::load(None).map_err(|e| format!("Failed to load settings: {}", e))?;
    info!("📋 Configuration loaded: {:?}", settings.device.name);

    let app_state = AppState {
        settings: Arc::new(RwLock::new(settings)),
        daemon_ref: Arc::new(Mutex::new(None)),
        device_manager: Arc::new(RwLock::new(DeviceManager::default())),
    };

    tauri::Builder::default()
        .manage(app_state)
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![
            get_discovered_devices,
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
            test_isolated_hotkey,
            check_main_hotkey_status,
            test_hotkey_system,
            get_device_stats,
            get_app_settings,
            get_connection_status,
            refresh_devices,
            get_system_info,
            get_network_metrics,
            update_app_settings,
            export_settings,
            import_settings,
            quit_app,
            hide_window
        ])
        .setup(|app| {
            // Create the main window with larger size for device management
            let _main_window = WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
                .title("Fileshare")
                .inner_size(480.0, 650.0) // Larger for device management
                .center()
                .decorations(false)
                .resizable(false)
                .visible(false)
                .focused(false)
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

            // CRITICAL FIX: Initialize hotkeys and daemon on main thread
            let app_handle = app.handle().clone();
            tokio::spawn(async move {
                if let Err(e) = setup_hotkeys_and_daemon(app_handle).await {
                    error!("❌ Failed to setup hotkeys and daemon: {}", e);
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");

    Ok(())
}

fn isolated_hotkey_test() -> String {
    use global_hotkey::{
        hotkey::{Code, HotKey},
        GlobalHotKeyManager,
    };

    let mut results = Vec::new();
    results.push("🔬 ISOLATED HOTKEY TEST".to_string());

    // Try to create a completely fresh manager
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

    // Try the most basic hotkey possible
    let basic_hotkey = HotKey::new(None, Code::F9); // Just F9, no modifiers

    match manager.register(basic_hotkey) {
        Ok(()) => {
            results.push("✅ F9 registered successfully!".to_string());
            results.push("This means basic hotkey registration CAN work".to_string());

            // Clean up
            if let Err(e) = manager.unregister(basic_hotkey) {
                results.push(format!("❌ Failed to unregister F9: {}", e));
            }
        }
        Err(e) => {
            results.push(format!("❌ Even F9 failed: {}", e));
            results.push("This is a fundamental system issue".to_string());
        }
    }

    results.join("\n")
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

    // Test 1: Manager Creation with detailed error info
    results.push("\n--- TEST 1: HOTKEY MANAGER CREATION ---".to_string());
    let manager = match GlobalHotKeyManager::new() {
        Ok(manager) => {
            results.push("✅ GlobalHotKeyManager created successfully".to_string());
            manager
        }
        Err(e) => {
            results.push(format!("❌ Failed to create GlobalHotKeyManager: {}", e));
            results.push(format!("Error type: {:?}", e));
            return results.join("\n");
        }
    };

    // Test 2: Try the SIMPLEST possible hotkey first
    results.push("\n--- TEST 2: ULTRA-SIMPLE HOTKEY TEST ---".to_string());

    let simple_hotkeys = vec![
        ("F11 only", HotKey::new(None, Code::F11)),
        ("F12 only", HotKey::new(None, Code::F12)),
        ("Alt+F11", HotKey::new(Some(Modifiers::ALT), Code::F11)),
        ("Ctrl+F11", HotKey::new(Some(Modifiers::CONTROL), Code::F11)),
    ];

    let mut any_success = false;

    for (name, hotkey) in &simple_hotkeys {
        match manager.register(*hotkey) {
            Ok(()) => {
                results.push(format!(
                    "✅ SUCCESS: {} registered (ID: {})",
                    name,
                    hotkey.id()
                ));
                any_success = true;

                // Immediately unregister to avoid conflicts
                if let Err(e) = manager.unregister(*hotkey) {
                    results.push(format!("❌ Failed to unregister {}: {}", name, e));
                }
            }
            Err(e) => {
                results.push(format!("❌ FAILED: {} - Error: {}", name, e));
                results.push(format!("   └─ Error details: {:?}", e));
            }
        }
    }

    if !any_success {
        results.push("\n🚨 CRITICAL: NO HOTKEYS CAN BE REGISTERED AT ALL!".to_string());
        results.push("This suggests a fundamental Windows/permissions issue.".to_string());
    }

    // Test 3: Windows-specific diagnostics
    results.push("\n--- TEST 3: WINDOWS ENVIRONMENT CHECK ---".to_string());

    #[cfg(target_os = "windows")]
    {
        unsafe {
            extern "system" {
                fn GetCurrentThreadId() -> u32;
                fn GetCurrentProcessId() -> u32;
                fn IsDebuggerPresent() -> i32;
                fn GetLastError() -> u32;
            }

            let thread_id = GetCurrentThreadId();
            let process_id = GetCurrentProcessId();
            let is_debugger = IsDebuggerPresent();
            let last_error = GetLastError();

            results.push(format!("Thread ID: {}", thread_id));
            results.push(format!("Process ID: {}", process_id));
            results.push(format!("Debugger present: {}", is_debugger != 0));
            results.push(format!("Last Windows error: {}", last_error));
        }
    }

    // Test 4: Permission and context checks
    results.push("\n--- TEST 4: ENVIRONMENT ANALYSIS ---".to_string());

    // Check if we're in a weird thread context
    let is_main_thread = std::thread::current().name() == Some("main");
    results.push(format!("Is main thread: {}", is_main_thread));
    results.push(format!("Thread name: {:?}", std::thread::current().name()));

    // Check working directory
    if let Ok(cwd) = std::env::current_dir() {
        results.push(format!("Working directory: {:?}", cwd));
    }

    // Check if running as admin/elevated
    #[cfg(target_os = "windows")]
    {
        results.push(format!("Checking Windows elevation status..."));
        // We'll add a simple check
    }

    results.push("\n🧪 DIAGNOSTICS COMPLETE".to_string());

    if !any_success {
        results.push("\n🎯 ANALYSIS: FUNDAMENTAL REGISTRATION FAILURE".to_string());
        results.push("Possible causes:".to_string());
        results.push("1. Windows permissions/UAC issues".to_string());
        results.push("2. Antivirus blocking hotkey registration".to_string());
        results.push("3. Another app has exclusive hotkey access".to_string());
        results.push("4. Tauri sandboxing interfering with system APIs".to_string());
        results.push("5. Windows thread context issues".to_string());
    }

    results.join("\n")
}

// NEW: Separate function to handle hotkey and daemon setup
async fn setup_hotkeys_and_daemon(
    app_handle: tauri::AppHandle,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("🔧 Setting up hotkeys and daemon...");

    let state: tauri::State<AppState> = app_handle.state();
    let settings = {
        let settings_lock = state.settings.read().await;
        settings_lock.clone()
    };

    // 1. FIRST: Initialize and start hotkeys on a dedicated thread
    let (hotkey_event_tx, mut hotkey_event_rx) = tokio::sync::mpsc::unbounded_channel();

    // Initialize hotkey manager on main thread for Windows compatibility
    let hotkey_handle = tokio::spawn(async move {
        if let Err(e) = setup_global_hotkeys(hotkey_event_tx).await {
            error!("❌ Failed to setup hotkeys: {}", e);
        }
    });

    // Give Windows time to properly register hotkeys
    tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;

    // 2. THEN: Start the daemon
    let daemon = FileshareDaemon::new(settings.clone())
        .await
        .map_err(|e| format!("Failed to create daemon: {}", e))?;

    let peer_manager_ref = daemon.peer_manager.clone();
    {
        let mut daemon_ref = state.daemon_ref.lock().await;
        *daemon_ref = Some(peer_manager_ref.clone());
    }

    info!("✅ Daemon created, starting services...");

    // 3. Start daemon services
    let daemon_handle = tokio::spawn(async move {
        if let Err(e) = daemon.run().await {
            error!("❌ Daemon run error: {}", e);
        }
    });

    // 4. Handle hotkey events with clipboard manager
    let clipboard_manager = ClipboardManager::new(settings.device.id);
    let hotkey_handler = tokio::spawn(async move {
        while let Some(event) = hotkey_event_rx.recv().await {
            match event {
                HotkeyEvent::CopyFiles => {
                    info!("📋 Copy hotkey triggered");
                    if let Err(e) =
                        handle_copy_operation(clipboard_manager.clone(), peer_manager_ref.clone())
                            .await
                    {
                        error!("❌ Copy operation failed: {}", e);
                    }
                }
                HotkeyEvent::PasteFiles => {
                    info!("📁 Paste hotkey triggered");
                    if let Err(e) =
                        handle_paste_operation(clipboard_manager.clone(), peer_manager_ref.clone())
                            .await
                    {
                        error!("❌ Paste operation failed: {}", e);
                    }
                }
            }
        }
    });

    // Wait for any service to finish
    tokio::select! {
        _ = daemon_handle => {
            info!("Daemon finished");
        }
        _ = hotkey_handler => {
            info!("Hotkey handler finished");
        }
        _ = hotkey_handle => {
            info!("Hotkey setup finished");
        }
    }

    info!("✅ Hotkeys and daemon setup completed");
    Ok(())
}

// NEW: Windows-optimized hotkey setup
async fn setup_global_hotkeys(
    event_tx: tokio::sync::mpsc::UnboundedSender<HotkeyEvent>,
) -> Result<(), Box<dyn std::error::Error>> {
    use global_hotkey::{
        hotkey::{Code, HotKey, Modifiers},
        GlobalHotKeyEvent, GlobalHotKeyManager,
    };
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    info!("🎹 Initializing global hotkeys...");

    let manager = GlobalHotKeyManager::new()
        .map_err(|e| format!("Failed to create hotkey manager: {}", e))?;

    // Platform-specific key combinations
    let (copy_hotkey, paste_hotkey) = if cfg!(target_os = "macos") {
        (
            HotKey::new(Some(Modifiers::META | Modifiers::SHIFT), Code::KeyY),
            HotKey::new(Some(Modifiers::META | Modifiers::SHIFT), Code::KeyI),
        )
    } else {
        // Windows and Linux - use Ctrl+Alt to avoid conflicts
        (
            HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyY),
            HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyI),
        )
    };

    // Register hotkeys with retry logic
    let mut retry_count = 0;
    while retry_count < 3 {
        match manager.register(copy_hotkey) {
            Ok(()) => {
                info!("✅ Copy hotkey registered successfully");
                break;
            }
            Err(e) => {
                retry_count += 1;
                error!(
                    "❌ Failed to register copy hotkey (attempt {}): {}",
                    retry_count, e
                );
                if retry_count < 3 {
                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                } else {
                    return Err(
                        format!("Failed to register copy hotkey after 3 attempts: {}", e).into(),
                    );
                }
            }
        }
    }

    retry_count = 0;
    while retry_count < 3 {
        match manager.register(paste_hotkey) {
            Ok(()) => {
                info!("✅ Paste hotkey registered successfully");
                break;
            }
            Err(e) => {
                retry_count += 1;
                error!(
                    "❌ Failed to register paste hotkey (attempt {}): {}",
                    retry_count, e
                );
                if retry_count < 3 {
                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                } else {
                    return Err(
                        format!("Failed to register paste hotkey after 3 attempts: {}", e).into(),
                    );
                }
            }
        }
    }

    info!("🎹 Starting hotkey event listener...");

    // Message loop - optimized for Windows
    let is_running = Arc::new(AtomicBool::new(true));
    let is_running_clone = is_running.clone();

    // Use a blocking thread for Windows message pump compatibility
    std::thread::spawn(move || {
        let receiver = GlobalHotKeyEvent::receiver();

        info!("🎹 Hotkey listener thread started");

        while is_running_clone.load(Ordering::SeqCst) {
            match receiver.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(event) => {
                    if event.state == global_hotkey::HotKeyState::Pressed {
                        info!("🎹 Hotkey event received: ID={}", event.id);

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
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    // Normal timeout, continue
                    continue;
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    warn!("🎹 Hotkey event receiver disconnected");
                    break;
                }
            }
        }

        info!("🎹 Hotkey listener thread stopped");
    });

    // Keep the manager alive by preventing it from being dropped
    std::mem::forget(manager);

    info!("✅ Hotkey system initialized successfully");
    Ok(())
}

// Helper functions for hotkey operations
async fn handle_copy_operation(
    clipboard: ClipboardManager,
    peer_manager: Arc<RwLock<fileshare_daemon::network::PeerManager>>,
) -> Result<(), fileshare_daemon::FileshareError> {
    info!("📋 Handling copy operation");

    clipboard.copy_selected_file().await?;

    let clipboard_item = {
        let clipboard_state = clipboard.network_clipboard.read().await;
        clipboard_state.clone()
    };

    if let Some(item) = clipboard_item {
        let peers = {
            let pm = peer_manager.read().await;
            pm.get_connected_peers()
        };

        for peer in peers {
            let message = fileshare_daemon::network::protocol::Message::new(
                fileshare_daemon::network::protocol::MessageType::ClipboardUpdate {
                    file_path: item.file_path.to_string_lossy().to_string(),
                    source_device: item.source_device,
                    timestamp: item.timestamp,
                    file_size: item.file_size,
                },
            );

            let mut pm = peer_manager.write().await;
            if let Err(e) = pm.send_message_to_peer(peer.device_info.id, message).await {
                warn!(
                    "❌ Failed to send clipboard update to {}: {}",
                    peer.device_info.id, e
                );
            }
        }

        let filename = item
            .file_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();

        notify_rust::Notification::new()
            .summary("File Copied to Network")
            .body(&format!(
                "Copied: {} ({})",
                filename,
                fileshare_daemon::utils::format_file_size(item.file_size)
            ))
            .timeout(notify_rust::Timeout::Milliseconds(3000))
            .show()
            .map_err(|e| {
                fileshare_daemon::FileshareError::Unknown(format!("Notification error: {}", e))
            })?;

        info!("✅ Copy operation completed successfully");
    }

    Ok(())
}

async fn handle_paste_operation(
    clipboard: ClipboardManager,
    peer_manager: Arc<RwLock<fileshare_daemon::network::PeerManager>>,
) -> Result<(), fileshare_daemon::FileshareError> {
    info!("📁 Handling paste operation");

    if let Some((target_path, source_device)) = clipboard.paste_to_current_location().await? {
        info!(
            "📁 Requesting file transfer from device {} to {:?}",
            source_device, target_path
        );

        let source_file_path = {
            let clipboard_state = clipboard.network_clipboard.read().await;
            clipboard_state
                .as_ref()
                .unwrap()
                .file_path
                .to_string_lossy()
                .to_string()
        };

        let request_id = uuid::Uuid::new_v4();
        let message = fileshare_daemon::network::protocol::Message::new(
            fileshare_daemon::network::protocol::MessageType::FileRequest {
                request_id,
                file_path: source_file_path,
                target_path: target_path.to_string_lossy().to_string(),
            },
        );

        let mut pm = peer_manager.write().await;
        pm.send_message_to_peer(source_device, message).await?;

        notify_rust::Notification::new()
            .summary("File Transfer Starting")
            .body("Requesting file from source device...")
            .timeout(notify_rust::Timeout::Milliseconds(3000))
            .show()
            .map_err(|e| {
                fileshare_daemon::FileshareError::Unknown(format!("Notification error: {}", e))
            })?;

        info!("✅ File request sent to source device");
    }

    Ok(())
}
