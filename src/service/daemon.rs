use crate::{
    clipboard::ClipboardManager,
    config::Settings,
    hotkeys::{HotkeyEvent, HotkeyManager},
    network::{DiscoveryService, PeerManager},
    tray::SystemTray,
    ui::{show_device_selector, show_file_picker},
    utils::format_file_size,
    Result,
};
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tracing::{error, info, warn};

pub struct FileshareDaemon {
    settings: Arc<Settings>,
    discovery: DiscoveryService,
    peer_manager: Arc<RwLock<PeerManager>>,
    tray: SystemTray,
    hotkey_manager: HotkeyManager,
    clipboard: ClipboardManager,
    shutdown_tx: broadcast::Sender<()>,
    shutdown_rx: broadcast::Receiver<()>,
}

impl FileshareDaemon {
    pub async fn new(settings: Settings) -> Result<Self> {
        let settings = Arc::new(settings);
        let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

        // Initialize peer manager
        let peer_manager = Arc::new(RwLock::new(PeerManager::new(settings.clone()).await?));

        // Initialize discovery service
        let discovery = DiscoveryService::new(
            settings.clone(),
            peer_manager.clone(),
            shutdown_tx.subscribe(),
        )
        .await?;

        // Initialize system tray
        let tray = SystemTray::new(settings.clone(), shutdown_tx.clone())?;

        // Initialize hotkey manager
        let hotkey_manager = HotkeyManager::new()?;

        // Initialize clipboard manager
        let clipboard = ClipboardManager::new();

        Ok(Self {
            settings,
            discovery,
            peer_manager,
            tray,
            hotkey_manager,
            clipboard,
            shutdown_tx,
            shutdown_rx,
        })
    }

    pub async fn run(mut self) -> Result<()> {
        info!("Starting Fileshare Daemon...");
        info!("Device ID: {}", self.settings.device.id);
        info!("Device Name: {}", self.settings.device.name);
        info!("Listening on port: {}", self.settings.network.port);

        // Start hotkey manager
        self.hotkey_manager.start().await?;

        // Start background services (NOT the tray)
        let discovery_handle = {
            let mut discovery = self.discovery.clone();
            tokio::spawn(async move {
                if let Err(e) = discovery.run().await {
                    error!("Discovery service error: {}", e);
                }
            })
        };

        let peer_manager_handle = {
            let peer_manager = self.peer_manager.clone();
            let settings = self.settings.clone();
            let mut shutdown_rx = self.shutdown_tx.subscribe();

            tokio::spawn(async move {
                tokio::select! {
                    result = Self::run_peer_manager(peer_manager, settings) => {
                        if let Err(e) = result {
                            error!("Peer manager error: {}", e);
                        }
                    }
                    _ = shutdown_rx.recv() => {
                        info!("Peer manager shutdown requested");
                    }
                }
            })
        };

        // Start hotkey event handler
        let hotkey_handle = {
            let peer_manager = self.peer_manager.clone();
            let clipboard = self.clipboard.clone();
            let mut hotkey_manager = self.hotkey_manager;
            let mut shutdown_rx = self.shutdown_tx.subscribe();

            tokio::spawn(async move {
                tokio::select! {
                    _ = Self::handle_hotkey_events(&mut hotkey_manager, peer_manager, clipboard) => {
                        info!("Hotkey handler stopped");
                    }
                    _ = shutdown_rx.recv() => {
                        info!("Hotkey handler shutdown requested");
                        if let Err(e) = hotkey_manager.stop() {
                            error!("Failed to stop hotkey manager: {}", e);
                        }
                    }
                }
            })
        };

        info!("Background services started successfully");

        // Run the system tray on the main thread (required for macOS)
        // Fixed the partial move issue here:
        let tray_result = {
            let mut tray = self.tray;
            let mut shutdown_rx = self.shutdown_rx;

            tokio::select! {
                result = tray.run() => {
                    // Handle the result properly without partial move
                    match result {
                        Ok(()) => {
                            info!("System tray stopped normally");
                            Ok(())
                        }
                        Err(e) => {
                            error!("System tray error: {}", e);
                            Err(e)  // Return the error
                        }
                    }
                }
                _ = shutdown_rx.recv() => {
                    info!("Shutdown signal received");
                    Ok(())
                }
            }
        };

        // Clean shutdown
        discovery_handle.abort();
        peer_manager_handle.abort();
        hotkey_handle.abort();

        info!("Fileshare Daemon stopped");
        tray_result
    }

    async fn handle_hotkey_events(
        hotkey_manager: &mut HotkeyManager,
        peer_manager: Arc<RwLock<PeerManager>>,
        clipboard: ClipboardManager,
    ) {
        info!("Starting hotkey event handler");

        loop {
            if let Some(event) = hotkey_manager.get_event().await {
                match event {
                    HotkeyEvent::CopyFiles => {
                        info!("Copy files hotkey triggered");
                        if let Err(e) = Self::handle_copy_files(clipboard.clone()).await {
                            error!("Failed to handle copy files: {}", e);
                        }
                    }
                    HotkeyEvent::PasteFiles => {
                        info!("Paste files hotkey triggered");
                        if let Err(e) =
                            Self::handle_paste_files(peer_manager.clone(), clipboard.clone()).await
                        {
                            error!("Failed to handle paste files: {}", e);
                        }
                    }
                }
            }
        }
    }

    async fn handle_copy_files(clipboard: ClipboardManager) -> Result<()> {
        info!("Handling copy files request");

        match show_file_picker().await? {
            Some(files) => {
                info!("User selected {} files to copy", files.len());

                // Store in clipboard
                clipboard.copy_files(files.clone()).await?;

                // Show notification with file details
                let total_size = {
                    let mut size = 0u64;
                    for file in &files {
                        if let Ok(metadata) = std::fs::metadata(file) {
                            size += metadata.len();
                        }
                    }
                    size
                };

                let message = if files.len() == 1 {
                    format!(
                        "Copied: {} ({})",
                        files[0].file_name().unwrap_or_default().to_string_lossy(),
                        format_file_size(total_size)
                    )
                } else {
                    format!(
                        "Copied {} files ({})",
                        files.len(),
                        format_file_size(total_size)
                    )
                };

                notify_rust::Notification::new()
                    .summary("Files Copied")
                    .body(&message)
                    .timeout(notify_rust::Timeout::Milliseconds(3000))
                    .show()
                    .map_err(|e| {
                        crate::FileshareError::Unknown(format!("Notification error: {}", e))
                    })?;

                info!(
                    "Files copied to clipboard: {} files, {} bytes",
                    files.len(),
                    total_size
                );
            }
            None => {
                info!("Copy files cancelled by user");
            }
        }

        Ok(())
    }

    async fn handle_paste_files(
        peer_manager: Arc<RwLock<PeerManager>>,
        clipboard: ClipboardManager,
    ) -> Result<()> {
        info!("Handling paste files request");

        // Check if clipboard has files
        if clipboard.is_empty().await {
            notify_rust::Notification::new()
                .summary("Nothing to Paste")
                .body("No files in clipboard. Use Cmd+Shift+Y to copy files first.")
                .timeout(notify_rust::Timeout::Milliseconds(4000))
                .show()
                .map_err(|e| {
                    crate::FileshareError::Unknown(format!("Notification error: {}", e))
                })?;
            return Ok(());
        }

        // Get connected peers
        let peers = {
            let pm = peer_manager.read().await;
            pm.get_connected_peers()
        };

        if peers.is_empty() {
            notify_rust::Notification::new()
                .summary("No Devices Available")
                .body("No connected devices found. Make sure other devices are running the app and connected to the same network.")
                .timeout(notify_rust::Timeout::Milliseconds(5000))
                .show()
                .map_err(|e| {
                    crate::FileshareError::Unknown(format!("Notification error: {}", e))
                })?;
            return Ok(());
        }

        match show_device_selector(peers).await? {
            Some(device_id) => {
                info!("User selected device: {}", device_id);

                // Get files from clipboard
                if let Some(clipboard_state) = clipboard.get_files().await {
                    info!(
                        "Starting file transfer: {} files to device {}",
                        clipboard_state.files.len(),
                        device_id
                    );

                    // Send each file
                    let mut sent_count = 0;
                    let total_files = clipboard_state.files.len();

                    for file_path in clipboard_state.files {
                        info!("Sending file: {:?}", file_path);

                        // Send file via peer manager
                        match {
                            let mut pm = peer_manager.write().await;
                            pm.send_file_to_peer(device_id, file_path.clone()).await
                        } {
                            Ok(()) => {
                                sent_count += 1;
                                info!("Successfully initiated transfer for: {:?}", file_path);
                            }
                            Err(e) => {
                                error!("Failed to send file {:?}: {}", file_path, e);
                                // Continue with other files
                            }
                        }
                    }

                    // Show completion notification
                    let message = if sent_count == total_files {
                        format!("Successfully sent {} files", sent_count)
                    } else {
                        format!("Sent {} of {} files", sent_count, total_files)
                    };

                    notify_rust::Notification::new()
                        .summary("Files Sent")
                        .body(&message)
                        .timeout(notify_rust::Timeout::Milliseconds(4000))
                        .show()
                        .map_err(|e| {
                            crate::FileshareError::Unknown(format!("Notification error: {}", e))
                        })?;

                    info!(
                        "File transfer completed: {} of {} files sent",
                        sent_count, total_files
                    );

                    // Clear clipboard after successful transfer
                    if sent_count > 0 {
                        clipboard.clear().await;
                    }
                }
            }
            None => {
                info!("Paste files cancelled or no devices available");
            }
        }

        Ok(())
    }

    async fn run_peer_manager(
        peer_manager: Arc<RwLock<PeerManager>>,
        _settings: Arc<Settings>,
    ) -> Result<()> {
        let listener = tokio::net::TcpListener::bind("0.0.0.0:9876").await?;
        info!("Peer manager listening on 0.0.0.0:9876");

        // Spawn a task to handle incoming connections
        let connection_pm = peer_manager.clone();
        let connection_handle = tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, addr)) => {
                        info!("New connection from {}", addr);
                        let pm = connection_pm.clone();

                        tokio::spawn(async move {
                            let mut pm = pm.write().await;
                            if let Err(e) = pm.handle_connection(stream).await {
                                warn!("Failed to handle connection from {}: {}", addr, e);
                            }
                        });
                    }
                    Err(e) => {
                        error!("Failed to accept connection: {}", e);
                        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                    }
                }
            }
        });

        // Spawn a task to process messages
        let message_pm = peer_manager.clone();
        let message_handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(100));
            loop {
                interval.tick().await;
                let mut pm = message_pm.write().await;
                if let Err(e) = pm.process_messages().await {
                    error!("Error processing messages: {}", e);
                }
            }
        });

        // Wait for either task to complete
        tokio::select! {
            _ = connection_handle => {},
            _ = message_handle => {},
        }

        Ok(())
    }
}
