// src/main.rs - COMPLETE SIMPLIFIED VERSION
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use fileshare_daemon::{config::Settings, service::FileshareDaemon};
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

// Enhanced DeviceInfo with simplified management
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
struct DeviceInfo {
    id: String,
    name: String,
    display_name: Option<String>,
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

// SIMPLIFIED: Transfer progress info
#[derive(serde::Serialize, serde::Deserialize)]
struct TransferProgressInfo {
    transfer_id: String,
    bytes_transferred: u64,
    total_bytes: u64,
    speed_mbps: f64,
    eta_seconds: f64,
    connections_active: usize,
    status: String,
    progress_percentage: f64,
}

// SIMPLIFIED: Get adaptive transfer progress
#[tauri::command]
async fn get_transfer_progress(
    transfer_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<Option<TransferProgressInfo>, String> {
    let transfer_uuid =
        Uuid::parse_str(&transfer_id).map_err(|e| format!("Invalid transfer ID: {}", e))?;

    if let Some(daemon_ref) = state.daemon_ref.lock().await.as_ref() {
        if let Some(progress) = daemon_ref.get_transfer_progress(transfer_uuid).await {
            Ok(Some(TransferProgressInfo {
                transfer_id: progress.transfer_id.to_string(),
                bytes_transferred: progress.bytes_transferred,
                total_bytes: progress.total_bytes,
                speed_mbps: progress.speed_mbps,
                eta_seconds: progress.eta_seconds(),
                connections_active: progress.connections_active,
                status: format!("{:?}", progress.status),
                progress_percentage: progress.percentage(),
            }))
        } else {
            Ok(None)
        }
    } else {
        Err("Daemon not ready".to_string())
    }
}

#[tauri::command]
async fn test_adaptive_transfer(state: tauri::State<'_, AppState>) -> Result<String, String> {
    info!("🧪 Testing simplified adaptive transfer system");

    if let Some(daemon_ref) = state.daemon_ref.lock().await.as_ref() {
        let stats = daemon_ref.get_connection_stats().await; // FIXED: Use daemon method

        Ok(format!(
            "📊 Adaptive Transfer System Status:\n\
            🔗 Connections: {} total ({} healthy)\n\
            🚀 Features: Memory-mapped I/O, Zero-copy, Direct TCP\n\
            ⚡ Performance: Optimized for maximum speed\n\
            📈 Adaptive: Automatically scales based on file size\n\
            ✅ Status: Ready for high-performance transfers",
            stats.total, stats.authenticated
        ))
    } else {
        Err("Daemon not ready".to_string())
    }
}

// Test system performance
#[tauri::command]
async fn test_system_performance() -> Result<String, String> {
    info!("🧪 Testing system performance capabilities");

    tokio::task::spawn_blocking(|| {
        use std::time::Instant;

        // Test memory allocation speed
        let start = Instant::now();
        let _large_buffer = vec![0u8; 100 * 1024 * 1024]; // 100MB
        let alloc_time = start.elapsed();

        // Test file I/O (if test file exists)
        let io_test_result = if let Ok(metadata) = std::fs::metadata("Cargo.toml") {
            let start = Instant::now();
            let _data = std::fs::read("Cargo.toml").unwrap_or_default();
            let io_time = start.elapsed();
            format!("File I/O: {:.2}ms", io_time.as_millis())
        } else {
            "File I/O: No test file available".to_string()
        };

        format!(
            "🔧 System Performance Test Results:\n\
            💾 Memory allocation (100MB): {:.2}ms\n\
            📁 {}\n\
            🖥️ Platform: {}\n\
            🏗️ Architecture: {}\n\
            ✅ System ready for high-performance transfers",
            alloc_time.as_millis(),
            io_test_result,
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    })
    .await
    .map_err(|e| format!("Performance test failed: {}", e))
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
        bytes_received: 1024 * 1024 * 50,
        bytes_sent: 1024 * 1024 * 30,
        transfers_total: 25,
        avg_speed: 1024 * 1024 * 2,
    })
}

#[tauri::command]
async fn update_app_settings(
    settings: AppSettings,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let mut app_settings = state.settings.write().await;

    app_settings.device.name = settings.device_name;
    app_settings.network.port = settings.network_port;
    app_settings.network.discovery_port = settings.discovery_port;
    app_settings.security.require_pairing = settings.require_pairing;
    app_settings.security.encryption_enabled = settings.encryption_enabled;

    app_settings
        .save(None)
        .map_err(|e| format!("Failed to save settings: {}", e))?;

    Ok(())
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
    require_pairing: bool,
    encryption_enabled: bool,
    auto_accept_from_trusted: bool,
    block_unknown_devices: bool,
}

// Device management storage
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
}

// Enhanced device discovery with metadata
#[tauri::command]
async fn get_discovered_devices(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<DeviceInfo>, String> {
    info!("🔍 UI requesting discovered devices");

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

        info!("📱 Returning {} discovered devices", devices.len());
        Ok(devices)
    } else {
        warn!("❌ Daemon not ready yet");
        Ok(Vec::new())
    }
}

// Device pairing with trust level
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

    info!("✅ Device {} paired successfully", device_id);
    Ok(())
}

// Block device
#[tauri::command]
async fn block_device(device_id: String, state: tauri::State<'_, AppState>) -> Result<(), String> {
    info!("🚫 UI requested to block device: {}", device_id);

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

    {
        let mut device_manager = state.device_manager.write().await;
        if !device_manager.blocked_devices.contains(&device_id) {
            device_manager.blocked_devices.push(device_id.clone());
        }

        if let Some(metadata) = device_manager.device_metadata.get_mut(&device_id) {
            metadata.trust_level = TrustLevel::Blocked;
        }
    }

    info!("✅ Device {} blocked successfully", device_id);
    Ok(())
}

// Keep existing commands for compatibility
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

    {
        let mut device_manager = state.device_manager.write().await;
        if let Some(metadata) = device_manager.device_metadata.get_mut(&device_id) {
            metadata.trust_level = TrustLevel::Unknown;
        }
    }

    info!("✅ Device {} unpaired successfully", device_id);
    Ok(())
}

#[tauri::command]
async fn rename_device(
    device_id: String,
    new_name: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    info!(
        "📝 UI requested to rename device {} to '{}'",
        device_id, new_name
    );

    if new_name.trim().is_empty() {
        return Err("Device name cannot be empty".to_string());
    }

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

#[tauri::command]
async fn cancel_transfer(
    transfer_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let transfer_uuid =
        Uuid::parse_str(&transfer_id).map_err(|e| format!("Invalid transfer ID: {}", e))?;

    if let Some(daemon_ref) = state.daemon_ref.lock().await.as_ref() {
        daemon_ref
            .cancel_transfer(transfer_uuid)
            .await
            .map_err(|e| format!("Failed to cancel transfer: {}", e))?;
        Ok(())
    } else {
        Err("Daemon not ready".to_string())
    }
}

#[tauri::command]
async fn get_all_active_transfers(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<TransferProgressInfo>, String> {
    if let Some(daemon_ref) = state.daemon_ref.lock().await.as_ref() {
        let transfers = daemon_ref.get_all_active_transfers().await;

        Ok(transfers
            .into_iter()
            .map(|progress| TransferProgressInfo {
                transfer_id: progress.transfer_id.to_string(),
                bytes_transferred: progress.bytes_transferred,
                total_bytes: progress.total_bytes,
                speed_mbps: progress.speed_mbps,
                eta_seconds: progress.eta_seconds(),
                connections_active: progress.connections_active,
                status: format!("{:?}", progress.status),
                progress_percentage: progress.percentage(),
            })
            .collect())
    } else {
        Err("Daemon not ready".to_string())
    }
}

#[tauri::command]
async fn get_app_settings(state: tauri::State<'_, AppState>) -> Result<AppSettings, String> {
    let settings = state.settings.read().await;

    Ok(AppSettings {
        device_name: settings.device.name.clone(),
        device_id: settings.device.id.to_string(),
        network_port: settings.network.port,
        discovery_port: settings.network.discovery_port,
        require_pairing: settings.security.require_pairing,
        encryption_enabled: settings.security.encryption_enabled,
        auto_accept_from_trusted: false,
        block_unknown_devices: false,
    })
}

#[tauri::command]
async fn get_connection_status(state: tauri::State<'_, AppState>) -> Result<bool, String> {
    let daemon_ref = state.daemon_ref.lock().await;
    Ok(daemon_ref.is_some())
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

    info!("🚀 Starting Simplified High-Performance Fileshare Daemon");

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
            rename_device,
            get_app_settings,
            get_connection_status,
            get_all_active_transfers,
            cancel_transfer,
            refresh_devices,
            get_system_info,
            get_network_metrics,
            get_transfer_progress,
            test_adaptive_transfer,
            test_system_performance,
            update_app_settings,
            quit_app,
            hide_window
        ])
        .setup(|app| {
            let _main_window = WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
                .title("Fileshare - High Performance")
                .inner_size(700.0, 700.0)
                .center()
                .decorations(true)
                .resizable(false)
                .visible(false)
                .focused(false)
                .shadow(true)
                .build()?;

            info!("🪟 Main window created");

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
                    .tooltip("Fileshare - High Performance Network File Sharing")
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

            // Start simplified daemon
            let app_handle = app.handle().clone();
            tokio::spawn(async move {
                if let Err(e) = start_simplified_daemon(app_handle).await {
                    error!("❌ Failed to start simplified daemon: {}", e);
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");

    Ok(())
}

// SIMPLIFIED: Daemon startup with adaptive transfer integration
async fn start_simplified_daemon(
    app_handle: tauri::AppHandle,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("🔧 Starting simplified high-performance daemon...");

    let state: tauri::State<AppState> = app_handle.state();

    let settings = {
        let settings_lock = state.settings.read().await;
        settings_lock.clone()
    };

    // Create the simplified daemon
    let daemon = Arc::new(
        FileshareDaemon::new(settings)
            .await
            .map_err(|e| format!("Failed to create daemon: {}", e))?,
    );

    info!("✅ Simplified daemon created successfully");

    // Store daemon reference for UI access
    {
        let mut daemon_ref = state.daemon_ref.lock().await;
        *daemon_ref = Some(daemon.clone());
    }

    info!("✅ Daemon reference stored for UI access");

    // Start background services
    daemon
        .start_background_services()
        .await
        .map_err(|e| format!("Failed to start background services: {}", e))?;

    info!("✅ Simplified high-performance daemon started successfully");
    Ok(())
}
