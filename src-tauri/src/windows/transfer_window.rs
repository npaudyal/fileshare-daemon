use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::{
    AppHandle, Emitter, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
    PhysicalPosition, Position,
};
use tokio::sync::RwLock;
use tracing::info;
use uuid::Uuid;

const TRANSFER_WINDOW_LABEL: &str = "transfer-progress";
const TRANSFER_WINDOW_WIDTH: f64 = 420.0;
const TRANSFER_WINDOW_HEIGHT: f64 = 500.0;
const TRANSFER_WINDOW_MIN_WIDTH: f64 = 350.0;
const TRANSFER_WINDOW_MIN_HEIGHT: f64 = 200.0;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WindowPosition {
    BottomRight,
    BottomLeft,
    TopRight,
    TopLeft,
    Center,
    Custom { x: i32, y: i32 },
    NearCursor,
}

impl Default for WindowPosition {
    fn default() -> Self {
        WindowPosition::BottomRight
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferWindowConfig {
    pub position: WindowPosition,
    pub auto_close_on_completion: bool,
    pub minimize_to_tray: bool,
    pub always_on_top: bool,
    pub transparent: bool,
    pub decorations: bool,
    pub resizable: bool,
    pub skip_taskbar: bool,
}

impl Default for TransferWindowConfig {
    fn default() -> Self {
        Self {
            position: WindowPosition::default(),
            auto_close_on_completion: true,
            minimize_to_tray: false,
            always_on_top: true,
            transparent: true,
            decorations: true,
            resizable: true,
            skip_taskbar: false,
        }
    }
}

pub struct TransferWindowManager {
    app_handle: AppHandle,
    config: Arc<RwLock<TransferWindowConfig>>,
    window_handle: Arc<RwLock<Option<WebviewWindow>>>,
}

impl TransferWindowManager {
    pub fn new(app_handle: AppHandle) -> Self {
        Self {
            app_handle,
            config: Arc::new(RwLock::new(TransferWindowConfig::default())),
            window_handle: Arc::new(RwLock::new(None)),
        }
    }

    /// Update window configuration
    pub async fn set_config(&self, config: TransferWindowConfig) {
        *self.config.write().await = config;
    }

    /// Get current window configuration
    pub async fn get_config(&self) -> TransferWindowConfig {
        self.config.read().await.clone()
    }

    /// Open or focus the transfer window
    pub async fn open_window(&self) -> Result<(), String> {
        let mut window_handle = self.window_handle.write().await;

        // Check if window already exists
        if let Some(ref window) = *window_handle {
            if window.is_visible().unwrap_or(false) {
                // Window exists and is visible, just focus it
                window.set_focus()
                    .map_err(|e| format!("Failed to focus window: {}", e))?;
                info!("Transfer window focused");
                return Ok(());
            } else {
                // Window exists but is hidden, show it
                window.show()
                    .map_err(|e| format!("Failed to show window: {}", e))?;
                window.set_focus()
                    .map_err(|e| format!("Failed to focus window: {}", e))?;
                info!("Transfer window shown and focused");
                return Ok(());
            }
        }

        // Create new window
        let config = self.config.read().await;
        let window = create_transfer_window(&self.app_handle, &config)?;

        *window_handle = Some(window);
        info!("Transfer window created");

        Ok(())
    }

    /// Close the transfer window
    pub async fn close_window(&self) -> Result<(), String> {
        let mut window_handle = self.window_handle.write().await;

        if let Some(window) = window_handle.take() {
            window.close()
                .map_err(|e| format!("Failed to close window: {}", e))?;
            info!("Transfer window closed");
        }

        Ok(())
    }

    /// Hide the transfer window
    pub async fn hide_window(&self) -> Result<(), String> {
        let window_handle = self.window_handle.read().await;

        if let Some(ref window) = *window_handle {
            window.hide()
                .map_err(|e| format!("Failed to hide window: {}", e))?;
            info!("Transfer window hidden");
        }

        Ok(())
    }

    /// Check if window is currently visible
    pub async fn is_visible(&self) -> bool {
        let window_handle = self.window_handle.read().await;

        if let Some(ref window) = *window_handle {
            window.is_visible().unwrap_or(false)
        } else {
            false
        }
    }

    /// Update window position
    pub async fn set_position(&self, position: WindowPosition) -> Result<(), String> {
        let window_handle = self.window_handle.read().await;

        if let Some(ref window) = *window_handle {
            apply_window_position(window, &position)?;

            // Update config
            let mut config = self.config.write().await;
            config.position = position;
        }

        Ok(())
    }
}

/// Create a new transfer window with the specified configuration
fn create_transfer_window(
    app_handle: &AppHandle,
    config: &TransferWindowConfig,
) -> Result<WebviewWindow, String> {
    // Build window with configuration
    let mut builder = WebviewWindowBuilder::new(
        app_handle,
        TRANSFER_WINDOW_LABEL,
        WebviewUrl::App("transfer.html".into()),
    )
    .title("Transfer Progress")
    .inner_size(TRANSFER_WINDOW_WIDTH, TRANSFER_WINDOW_HEIGHT)
    .min_inner_size(TRANSFER_WINDOW_MIN_WIDTH, TRANSFER_WINDOW_MIN_HEIGHT)
    .resizable(config.resizable)
    .decorations(config.decorations)
    .always_on_top(config.always_on_top)
    .skip_taskbar(config.skip_taskbar)
    .visible(false) // Start hidden, then position and show
    .focused(true);

    // Set transparency/vibrancy for native feel
    #[cfg(target_os = "macos")]
    {
        builder = builder
            .hidden_title(true)
            .title_bar_style(tauri::TitleBarStyle::Overlay);
    }

    // Build the window
    let window = builder
        .build()
        .map_err(|e| format!("Failed to create window: {}", e))?;

    // Apply position after creation
    apply_window_position(&window, &config.position)?;

    // Show the window after positioning
    window.show()
        .map_err(|e| format!("Failed to show window: {}", e))?;

    Ok(window)
}

/// Apply the specified position to the window
fn apply_window_position(
    window: &WebviewWindow,
    position: &WindowPosition,
) -> Result<(), String> {
    match position {
        WindowPosition::BottomRight => {
            position_bottom_right(window)?;
        }
        WindowPosition::BottomLeft => {
            position_bottom_left(window)?;
        }
        WindowPosition::TopRight => {
            position_top_right(window)?;
        }
        WindowPosition::TopLeft => {
            position_top_left(window)?;
        }
        WindowPosition::Center => {
            window.center()
                .map_err(|e| format!("Failed to center window: {}", e))?;
        }
        WindowPosition::Custom { x, y } => {
            window.set_position(Position::Physical(PhysicalPosition::new(*x, *y)))
                .map_err(|e| format!("Failed to set custom position: {}", e))?;
        }
        WindowPosition::NearCursor => {
            position_near_cursor(window)?;
        }
    }

    Ok(())
}

/// Position window in the bottom-right corner
fn position_bottom_right(window: &WebviewWindow) -> Result<(), String> {
    let monitor = window.current_monitor()
        .map_err(|e| format!("Failed to get monitor: {}", e))?
        .ok_or("No monitor found")?;

    let monitor_size = monitor.size();
    let window_size = window.outer_size()
        .map_err(|e| format!("Failed to get window size: {}", e))?;

    let x = monitor_size.width as i32 - window_size.width as i32 - 20;
    let y = monitor_size.height as i32 - window_size.height as i32 - 40;

    window.set_position(Position::Physical(PhysicalPosition::new(x, y)))
        .map_err(|e| format!("Failed to set position: {}", e))?;

    Ok(())
}

/// Position window in the bottom-left corner
fn position_bottom_left(window: &WebviewWindow) -> Result<(), String> {
    let monitor = window.current_monitor()
        .map_err(|e| format!("Failed to get monitor: {}", e))?
        .ok_or("No monitor found")?;

    let monitor_size = monitor.size();
    let window_size = window.outer_size()
        .map_err(|e| format!("Failed to get window size: {}", e))?;

    let x = 20;
    let y = monitor_size.height as i32 - window_size.height as i32 - 40;

    window.set_position(Position::Physical(PhysicalPosition::new(x, y)))
        .map_err(|e| format!("Failed to set position: {}", e))?;

    Ok(())
}

/// Position window in the top-right corner
fn position_top_right(window: &WebviewWindow) -> Result<(), String> {
    let monitor = window.current_monitor()
        .map_err(|e| format!("Failed to get monitor: {}", e))?
        .ok_or("No monitor found")?;

    let monitor_size = monitor.size();
    let window_size = window.outer_size()
        .map_err(|e| format!("Failed to get window size: {}", e))?;

    let x = monitor_size.width as i32 - window_size.width as i32 - 20;
    let y = 40;

    window.set_position(Position::Physical(PhysicalPosition::new(x, y)))
        .map_err(|e| format!("Failed to set position: {}", e))?;

    Ok(())
}

/// Position window in the top-left corner
fn position_top_left(window: &WebviewWindow) -> Result<(), String> {
    let x = 20;
    let y = 40;

    window.set_position(Position::Physical(PhysicalPosition::new(x, y)))
        .map_err(|e| format!("Failed to set position: {}", e))?;

    Ok(())
}

/// Position window near the cursor (for context-aware positioning)
fn position_near_cursor(window: &WebviewWindow) -> Result<(), String> {
    // For now, just center it. In the future, we could get cursor position
    // through platform-specific APIs
    window.center()
        .map_err(|e| format!("Failed to center window: {}", e))?;

    Ok(())
}

/// Convenience function to spawn a transfer window
pub async fn spawn_transfer_window(
    app_handle: AppHandle,
    transfer_id: Option<Uuid>,
) -> Result<(), String> {
    let manager = TransferWindowManager::new(app_handle);
    manager.open_window().await?;

    // If a specific transfer ID is provided, we can emit an event to focus on it
    if let Some(id) = transfer_id {
        if let Some(window) = manager.window_handle.read().await.as_ref() {
            window.emit("focus-transfer", id)
                .map_err(|e| format!("Failed to emit focus event: {}", e))?;
        }
    }

    Ok(())
}