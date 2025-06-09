// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use fileshare_daemon::{config::Settings, service::FileshareDaemon};
use std::sync::Arc;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, TrayIconBuilder, TrayIconEvent},
    Manager, WebviewUrl, WebviewWindowBuilder,
};
use tokio::sync::{Mutex, RwLock};
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
    // Store settings separately since daemon will be moved
    settings: Arc<RwLock<Settings>>,
    // Store daemon reference for accessing peer manager
    daemon_ref: Arc<Mutex<Option<Arc<RwLock<fileshare_daemon::network::PeerManager>>>>>,
}

// FIXED: Get real discovered devices through peer manager
#[tauri::command]
async fn get_discovered_devices(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<DeviceInfo>, String> {
    info!("🔍 UI requesting discovered devices");

    let settings = state.settings.read().await;

    // Get peer manager reference
    if let Some(peer_manager_ref) = state.daemon_ref.lock().await.as_ref() {
        let pm = peer_manager_ref.read().await;

        // Get discovered devices from peer manager
        let discovered = pm.get_all_discovered_devices().await;

        let mut devices = Vec::new();

        for device in discovered {
            // Check if device is paired (in allowed list)
            let is_paired = settings.security.allowed_devices.contains(&device.id);

            // Check if device is currently connected
            let is_connected = pm
                .get_connected_peers()
                .iter()
                .any(|p| p.device_info.id == device.id);

            // Determine device type based on name
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
        warn!("❌ Peer manager not ready yet");
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
    if let Some(peer_manager_ref) = state.daemon_ref.lock().await.as_ref() {
        let mut pm = peer_manager_ref.write().await;
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
    // Discovery runs continuously, so just return success
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
        settings: Arc::new(RwLock::new(settings)),
        daemon_ref: Arc::new(Mutex::new(None)),
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
            get_connection_status,
            refresh_devices,
            quit_app,
            hide_window
        ])
        .setup(|app| {
            // Create the main window
            let _main_window = WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
                .title("Fileshare")
                .inner_size(420.0, 580.0)
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

            // Start daemon in background
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

    // Store reference to peer manager before starting daemon
    let peer_manager_ref = daemon.peer_manager.clone();
    {
        let mut daemon_ref = state.daemon_ref.lock().await;
        *daemon_ref = Some(peer_manager_ref);
    }

    info!("✅ Daemon created, starting services...");

    // Start daemon (this will move the daemon)
    tokio::spawn(async move {
        if let Err(e) = daemon.run().await {
            error!("❌ Daemon run error: {}", e);
        }
    });

    info!("✅ Background daemon started successfully");
    Ok(())
}
