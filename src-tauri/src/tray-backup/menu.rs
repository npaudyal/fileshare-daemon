use crate::{config::Settings, FileshareError, Result};
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, info};
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    TrayIconBuilder,
};
use winit::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
};

#[derive(Debug, Clone)]
pub enum TrayEvent {
    ShowDevices,
    OpenSettings,
    Quit,
}

#[derive(Clone)]
pub struct SystemTray {
    settings: Arc<Settings>,
    shutdown_tx: broadcast::Sender<()>,
}

impl SystemTray {
    pub fn new(settings: Arc<Settings>, shutdown_tx: broadcast::Sender<()>) -> Result<Self> {
        Ok(Self {
            settings,
            shutdown_tx,
        })
    }

    pub async fn run(&mut self) -> Result<()> {
        info!("Starting system tray");

        // Create event loop for tray
        let event_loop = EventLoop::new()
            .map_err(|e| FileshareError::Tray(format!("Failed to create event loop: {}", e)))?;

        // Create tray menu
        let tray_menu = self.create_menu()?;

        // Load icon
        let icon = self.load_icon()?;

        // Create tray icon
        let _tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(tray_menu))
            .with_tooltip("Fileshare Daemon")
            .with_icon(icon)
            .build()
            .map_err(|e| FileshareError::Tray(format!("Failed to create tray icon: {}", e)))?;

        info!("System tray created successfully");

        // Handle menu events
        let menu_channel = MenuEvent::receiver();
        let shutdown_tx = self.shutdown_tx.clone();

        // Run event loop
        event_loop
            .run(move |event, elwt| {
                elwt.set_control_flow(ControlFlow::Wait);

                // Check for menu events
                if let Ok(menu_event) = menu_channel.try_recv() {
                    debug!("Menu event received: {:?}", menu_event.id);
                    match TrayEvent::from_menu_id(&menu_event.id) {
                        TrayEvent::ShowDevices => {
                            info!("Show devices requested");
                            // TODO: Show devices window or notification
                        }
                        TrayEvent::OpenSettings => {
                            info!("Open settings requested");
                            // TODO: Open settings window
                        }
                        TrayEvent::Quit => {
                            info!("Quit requested from tray");
                            let _ = shutdown_tx.send(());
                            elwt.exit();
                        }
                    }
                }

                match event {
                    Event::WindowEvent {
                        event: WindowEvent::CloseRequested,
                        ..
                    } => {
                        elwt.exit();
                    }
                    _ => {}
                }
            })
            .map_err(|e| FileshareError::Tray(format!("Event loop error: {}", e)))?;

        info!("System tray stopped");
        Ok(())
    }

    fn create_menu(&self) -> Result<Menu> {
        let menu = Menu::new();

        // Device info
        let device_info = MenuItem::new(
            format!("Device: {}", self.settings.device.name),
            false,
            None,
        );

        // Show devices
        let show_devices = MenuItem::new("Show Devices", true, None);

        // Settings
        let settings = MenuItem::new("Settings", true, None);

        // Separator
        let separator = PredefinedMenuItem::separator();

        // Quit
        let quit = PredefinedMenuItem::quit(Some("Quit"));

        menu.append_items(&[
            &device_info,
            &separator,
            &show_devices,
            &settings,
            &separator,
            &quit,
        ])
        .map_err(|e| FileshareError::Tray(format!("Failed to create menu: {}", e)))?;

        Ok(menu)
    }

    fn load_icon(&self) -> Result<tray_icon::Icon> {
        // Create a simple 32x32 icon programmatically
        let icon_size: u32 = 32; // Changed to u32
        let mut icon_data = vec![0u8; (icon_size * icon_size * 4) as usize]; // Convert to usize for vec allocation

        // Create a simple blue circle icon
        for y in 0..icon_size {
            for x in 0..icon_size {
                let idx = ((y * icon_size + x) * 4) as usize; // Convert to usize for indexing
                let center_x = icon_size as f32 / 2.0;
                let center_y = icon_size as f32 / 2.0;
                let distance =
                    ((x as f32 - center_x).powi(2) + (y as f32 - center_y).powi(2)).sqrt();

                if distance <= center_x - 2.0 {
                    // Blue circle
                    icon_data[idx] = 59; // R
                    icon_data[idx + 1] = 130; // G
                    icon_data[idx + 2] = 246; // B
                    icon_data[idx + 3] = 255; // A
                } else {
                    // Transparent
                    icon_data[idx] = 0;
                    icon_data[idx + 1] = 0;
                    icon_data[idx + 2] = 0;
                    icon_data[idx + 3] = 0;
                }
            }
        }

        // Now icon_size is u32, so no conversion needed
        let icon = tray_icon::Icon::from_rgba(icon_data, icon_size, icon_size)
            .map_err(|e| FileshareError::Tray(format!("Failed to create icon: {}", e)))?;

        Ok(icon)
    }
}

impl TrayEvent {
    fn from_menu_id(id: &tray_icon::menu::MenuId) -> Self {
        match id.0.as_str() {
            "Show Devices" => TrayEvent::ShowDevices,
            "Settings" => TrayEvent::OpenSettings,
            _ => TrayEvent::Quit,
        }
    }
}
