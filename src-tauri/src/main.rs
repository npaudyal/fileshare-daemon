// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use fileshare_daemon::{config::Settings, service::FileshareDaemon};
use std::sync::Arc;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, TrayIconBuilder, TrayIconEvent},
    Manager, WebviewUrl, WebviewWindowBuilder,
};
use tokio::sync::RwLock;
use tracing::{error, info, warn};
use uuid::Uuid;

// Enhanced DeviceInfo with connection status
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
struct DeviceInfo {
    id: String,
    name: String,
    device_type: String,
    is_paired: bool,
    is_connected: bool,
    last_seen: u64,
    address: String,
    version: String,
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
}

struct AppState {
    daemon: Arc<RwLock<Option<FileshareDaemon>>>,
    settings: Arc<RwLock<Settings>>,
}

// FIXED: Get real discovered devices from daemon
#[tauri::command]
async fn get_discovered_devices(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<DeviceInfo>, String> {
    info!("🔍 UI requesting discovered devices");

    let settings = state.settings.read().await;

    // Get daemon and discovery service
    if let Some(daemon) = state.daemon.read().await.as_ref() {
        // Access the discovery service from daemon
        let discovered = daemon.discovery.get_discovered_devices().await;

        let mut devices = Vec::new();

        for device in discovered {
            // Check if device is paired (in allowed list)
            let is_paired = settings.security.allowed_devices.contains(&device.id);

            // Check if device is currently connected via peer manager
            let is_connected = {
                let pm = daemon.peer_manager.read().await;
                pm.get_connected_peers()
                    .iter()
                    .any(|p| p.device_info.id == device.id)
            };

            // Determine device type based on name or other heuristics
            let device_type = determine_device_type(&device.name);

            devices.push(DeviceInfo {
                id: device.id.to_string(),
                name: device.name,
                device_type,
                is_paired,
                is_connected,
                last_seen: device.last_seen,
                address: device.addr.to_string(),
                version: device.version,
            });
        }

        info!("📱 Returning {} discovered devices to UI", devices.len());
        Ok(devices)
    } else {
        warn!("❌ Daemon not ready yet");
        Ok(Vec::new())
    }
}

#[tauri::command]
async fn pair_device(device_id: String, state: tauri::State<'_, AppState>) -> Result<(), String> {
    info!("🔗 UI requested to pair device: {}", device_id);

    let device_uuid =
        Uuid::parse_str(&device_id).map_err(|e| format!("Invalid device ID: {}", e))?;

    // Add device to allowed list
    {
        let mut settings = state.settings.write().await;
        if !settings.security.allowed_devices.contains(&device_uuid) {
            settings.security.allowed_devices.push(device_uuid);

            // Save settings
            if let Err(e) = settings.save(None) {
                error!("Failed to save settings: {}", e);
                return Err(format!("Failed to save settings: {}", e));
            }
        }
    }

    // Try to connect to the device
    if let Some(daemon) = state.daemon.read().await.as_ref() {
        let mut pm = daemon.peer_manager.write().await;
        if let Err(e) = pm.connect_to_peer(device_uuid).await {
            warn!("Failed to connect to paired device {}: {}", device_id, e);
            // Don't return error - pairing succeeded, connection might happen later
        }
    }

    info!("✅ Device {} paired successfully", device_id);
    Ok(())
}

#[tauri::command]
async fn unpair_device(device_id: String, state: tauri::State<'_, AppState>) -> Result<(), String> {
    info!("🔓 UI requested to unpair device: {}", device_id);

    let device_uuid =
        Uuid::parse_str(&device_id).map_err(|e| format!("Invalid device ID: {}", e))?;

    // Remove device from allowed list
    {
        let mut settings = state.settings.write().await;
        settings
            .security
            .allowed_devices
            .retain(|&id| id != device_uuid);

        // Save settings
        if let Err(e) = settings.save(None) {
            error!("Failed to save settings: {}", e);
            return Err(format!("Failed to save settings: {}", e));
        }
    }

    info!("✅ Device {} unpaired successfully", device_id);
    Ok(())
}

#[tauri::command]
async fn rename_device(
    device_id: String,
    new_name: String,
    _state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    info!(
        "📝 UI requested to rename device {} to {}",
        device_id, new_name
    );
    // TODO: Implement device renaming in backend
    // For now, just log the request
    Ok(())
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
    })
}

#[tauri::command]
async fn update_settings(
    new_settings: AppSettings,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    info!("⚙️ UI requested to update settings");

    {
        let mut settings = state.settings.write().await;

        // Update settings
        settings.device.name = new_settings.device_name;
        settings.network.port = new_settings.network_port;
        settings.network.discovery_port = new_settings.discovery_port;
        settings.transfer.chunk_size = new_settings.chunk_size;
        settings.transfer.max_concurrent_transfers = new_settings.max_concurrent_transfers;
        settings.security.require_pairing = new_settings.require_pairing;
        settings.security.encryption_enabled = new_settings.encryption_enabled;

        // Save to file
        if let Err(e) = settings.save(None) {
            error!("Failed to save settings: {}", e);
            return Err(format!("Failed to save settings: {}", e));
        }
    }

    info!("✅ Settings updated successfully");
    Ok(())
}

#[tauri::command]
async fn get_connection_status(state: tauri::State<'_, AppState>) -> Result<bool, String> {
    if let Some(daemon) = state.daemon.read().await.as_ref() {
        let pm = daemon.peer_manager.read().await;
        let connected_count = pm.get_connected_peers().len();
        Ok(connected_count > 0)
    } else {
        Ok(false)
    }
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

#[tauri::command]
async fn refresh_devices(state: tauri::State<'_, AppState>) -> Result<(), String> {
    info!("🔄 UI requested device refresh");
    // The discovery service runs continuously, so just return success
    // Real refreshing happens automatically via the discovery service
    Ok(())
}

// Helper function to determine device type
fn determine_device_type(device_name: &str) -> String {
    let name_lower = device_name.to_lowercase();

    if name_lower.contains("iphone")
        || name_lower.contains("android")
        || name_lower.contains("mobile")
    {
        "phone".to_string()
    } else if name_lower.contains("macbook") || name_lower.contains("laptop") {
        "laptop".to_string()
    } else {
        "desktop".to_string()
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter("fileshare_daemon=debug,fileshare_daemon::network::discovery=debug")
        .init();

    info!("🚀 Starting Fileshare Daemon with Tauri v2 UI");

    let settings = Settings::load(None).map_err(|e| format!("Failed to load settings: {}", e))?;
    info!("📋 Configuration loaded: {:?}", settings.device.name);

    let app_state = AppState {
        daemon: Arc::new(RwLock::new(None)),
        settings: Arc::new(RwLock::new(settings)),
    };

    tauri::Builder::default()
        .manage(app_state)
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![
            get_discovered_devices,
            pair_device,
            unpair_device,
            rename_device,
            get_app_settings,
            update_settings,
            get_connection_status,
            refresh_devices,
            quit_app,
            hide_window
        ])
        .setup(|app| {
            // Create the main window immediately but keep it hidden
            let _main_window = WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
                .title("Fileshare")
                .inner_size(420.0, 580.0) // Slightly larger for better UX
                .center()
                .decorations(false)
                .resizable(false)
                .visible(false)
                .focused(false)
                .build()?;

            info!("🪟 Main window created and hidden");

            // Create tray menu items
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let show_hide =
                MenuItem::with_id(app, "show_hide", "Show/Hide Window", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_hide, &quit])?;

            // Get app handle for use in closures
            let app_handle_for_tray = app.handle().clone();
            let app_handle_for_menu = app.handle().clone();

            // Create system tray with menu
            let tray_result = if let Some(icon) = app.default_window_icon() {
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
                                    info!("🙈 Window hidden");
                                } else {
                                    let _ = window.show();
                                    let _ = window.center();
                                    let _ = window.set_focus();
                                    info!("👁️ Window shown and focused");
                                }
                            }
                        }
                        _ => {}
                    })
                    .on_menu_event(move |_tray, event| match event.id.as_ref() {
                        "quit" => {
                            std::process::exit(0);
                        }
                        "show_hide" => {
                            if let Some(window) = app_handle_for_menu.get_webview_window("main") {
                                let is_visible = window.is_visible().unwrap_or(false);
                                if is_visible {
                                    let _ = window.hide();
                                    info!("🙈 Window hidden via menu");
                                } else {
                                    let _ = window.show();
                                    let _ = window.center();
                                    let _ = window.set_focus();
                                    info!("👁️ Window shown and focused via menu");
                                }
                            }
                        }
                        _ => {}
                    })
                    .build(app)
            } else {
                error!("❌ No default icon available for tray");
                return Err("No tray icon available".into());
            };

            match tray_result {
                Ok(_) => {
                    info!("✅ System tray icon created successfully");
                }
                Err(e) => {
                    error!("❌ Failed to create tray icon: {}", e);
                    return Err(e.into());
                }
            }

            // Start the daemon in background
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

async fn start_daemon(app_handle: tauri::AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    info!("🔧 Starting background daemon...");

    let state: tauri::State<AppState> = app_handle.state();
    let settings = {
        let settings_lock = state.settings.read().await;
        settings_lock.clone()
    };

    let daemon = FileshareDaemon::new(settings)
        .await
        .map_err(|e| format!("Failed to create daemon: {}", e))?;

    // Store daemon in state
    {
        let mut daemon_lock = state.daemon.write().await;
        *daemon_lock = Some(daemon);
    }

    info!("✅ Background daemon started successfully");
    Ok(())
}
