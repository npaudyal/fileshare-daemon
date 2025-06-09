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
use tracing::{error, info};
use uuid::Uuid;

// Tauri commands for v2
#[tauri::command]
async fn get_discovered_devices(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<DeviceInfo>, String> {
    let _daemon = state.daemon.read().await;
    // Sample data for testing
    Ok(vec![
        DeviceInfo {
            id: "test-device-1".to_string(),
            name: "iPhone".to_string(),
            device_type: "phone".to_string(),
            is_paired: true,
            is_connected: true,
            last_seen: 1234567890,
        },
        DeviceInfo {
            id: "test-device-2".to_string(),
            name: "Windows PC".to_string(),
            device_type: "desktop".to_string(),
            is_paired: false,
            is_connected: false,
            last_seen: 1234567890,
        },
    ])
}

#[tauri::command]
async fn pair_device(device_id: String, _state: tauri::State<'_, AppState>) -> Result<(), String> {
    info!("Pairing with device: {}", device_id);
    Ok(())
}

#[tauri::command]
async fn unpair_device(
    device_id: String,
    _state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    info!("Unpairing device: {}", device_id);
    Ok(())
}

#[tauri::command]
async fn rename_device(
    device_id: String,
    new_name: String,
    _state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    info!("Renaming device {} to {}", device_id, new_name);
    Ok(())
}

#[tauri::command]
async fn get_app_settings(_state: tauri::State<'_, AppState>) -> Result<AppSettings, String> {
    Ok(AppSettings {
        device_name: "My MacBook".to_string(),
        device_id: Uuid::new_v4().to_string(),
        network_port: 9876,
        discovery_port: 9877,
        chunk_size: 65536,
        max_concurrent_transfers: 5,
    })
}

#[tauri::command]
async fn quit_app(app: tauri::AppHandle) -> Result<(), String> {
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

#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct DeviceInfo {
    id: String,
    name: String,
    device_type: String,
    is_paired: bool,
    is_connected: bool,
    last_seen: u64,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct AppSettings {
    device_name: String,
    device_id: String,
    network_port: u16,
    discovery_port: u16,
    chunk_size: usize,
    max_concurrent_transfers: usize,
}

struct AppState {
    daemon: Arc<RwLock<Option<FileshareDaemon>>>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter("fileshare_daemon=debug,fileshare_daemon::network::discovery=debug")
        .init();

    info!("Starting Fileshare Daemon with Tauri v2 UI");

    let settings = Settings::load(None).map_err(|e| format!("Failed to load settings: {}", e))?;
    info!("Configuration loaded: {:?}", settings.device.name);

    let app_state = AppState {
        daemon: Arc::new(RwLock::new(None)),
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
            quit_app,
            hide_window
        ])
        .setup(|app| {
            // Create the main window immediately but keep it hidden
            let _main_window = WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
                .title("Fileshare")
                .inner_size(400.0, 500.0)
                .center()
                .decorations(false)
                .resizable(false)
                .visible(false)
                .focused(false) // Don't steal focus on creation
                .build()?;

            info!("Main window created and hidden");

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
                    .on_tray_icon_event(move |_tray, event| {
                        match event {
                            TrayIconEvent::Click {
                                button: MouseButton::Left,
                                ..
                            } => {
                                // Toggle window on left click
                                if let Some(window) = app_handle_for_tray.get_webview_window("main")
                                {
                                    let is_visible = window.is_visible().unwrap_or(false);

                                    if is_visible {
                                        let _ = window.hide();
                                        info!("Window hidden");
                                    } else {
                                        let _ = window.show();
                                        let _ = window.center();
                                        let _ = window.set_focus();
                                        info!("Window shown and focused");
                                    }
                                }
                            }
                            _ => {}
                        }
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
                                    info!("Window hidden via menu");
                                } else {
                                    let _ = window.show();
                                    let _ = window.center();
                                    let _ = window.set_focus();
                                    info!("Window shown and focused via menu");
                                }
                            }
                        }
                        _ => {}
                    })
                    .build(app)
            } else {
                error!("No default icon available for tray");
                return Err("No tray icon available".into());
            };

            match tray_result {
                Ok(_) => {
                    info!("System tray icon created successfully");
                }
                Err(e) => {
                    error!("Failed to create tray icon: {}", e);
                    return Err(e.into());
                }
            }

            // Start the daemon in background
            let app_handle = app.handle().clone();
            tokio::spawn(async move {
                if let Err(e) = start_daemon(app_handle).await {
                    error!("Failed to start daemon: {}", e);
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");

    Ok(())
}

async fn start_daemon(app_handle: tauri::AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    info!("Starting background daemon...");

    let settings = Settings::load(None).map_err(|e| format!("Failed to load settings: {}", e))?;

    let daemon = FileshareDaemon::new(settings)
        .await
        .map_err(|e| format!("Failed to create daemon: {}", e))?;

    let state: tauri::State<AppState> = app_handle.state();
    let mut daemon_lock = state.daemon.write().await;
    *daemon_lock = Some(daemon);

    info!("Background daemon started successfully");
    Ok(())
}
