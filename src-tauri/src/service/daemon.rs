use crate::{
    clipboard::ClipboardManager,
    config::Settings,
    hotkeys::{HotkeyEvent, HotkeyManager},
    network::{DiscoveryService, PeerManager},
    Result,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, RwLock};
use tracing::{error, info, warn};

pub struct FileshareDaemon {
    settings: Arc<Settings>,
    pub discovery: Option<DiscoveryService>,
    pub peer_manager: Arc<RwLock<PeerManager>>,
    hotkey_manager: Option<HotkeyManager>,
    clipboard: ClipboardManager,
    shutdown_tx: broadcast::Sender<()>,
    shutdown_rx: broadcast::Receiver<()>,
}

impl FileshareDaemon {
    pub async fn new(settings: Settings) -> Result<Self> {
        let settings = Arc::new(settings);
        let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

        // Initialize peer manager with streaming support
        let peer_manager = PeerManager::new(settings.clone()).await?;
        let peer_manager = Arc::new(RwLock::new(peer_manager));

        // Initialize discovery service
        let discovery = DiscoveryService::new(
            settings.clone(),
            peer_manager.clone(),
            shutdown_tx.subscribe(),
        )
        .await?;

        // Initialize hotkey manager
        let hotkey_manager = HotkeyManager::new()?;

        // Initialize clipboard manager with device ID
        let clipboard = ClipboardManager::new(settings.device.id);

        info!("🚀 FileshareDaemon initialized with intelligent transfer selection");
        info!("📊 Transfer Logic: <50MB = Chunked Protocol, ≥50MB = Streaming Protocol");

        Ok(Self {
            settings,
            discovery: Some(discovery),
            peer_manager,
            hotkey_manager: Some(hotkey_manager),
            clipboard,
            shutdown_tx,
            shutdown_rx,
        })
    }

    // Method to get discovered devices (for UI access)
    pub async fn get_discovered_devices(&self) -> Vec<crate::network::discovery::DeviceInfo> {
        let pm = self.peer_manager.read().await;
        pm.get_all_discovered_devices().await
    }

    // ✅ FIXED: Enhanced daemon startup with proper streaming integration
    pub async fn start_background_services(self: Arc<Self>) -> Result<()> {
        info!("🚀 Starting Enhanced Fileshare Daemon with Intelligent Transfer Selection...");
        info!("📱 Device ID: {}", self.settings.device.id);
        info!("🏷️ Device Name: {}", self.settings.device.name);
        info!("🌐 Control port: {}", self.settings.network.port);
        info!("🚀 Streaming port: {}", self.settings.network.port + 1);
        info!("🧠 Smart Selection: Auto-chooses chunked vs streaming based on file size");

        // ✅ FIXED: Start streaming listener FIRST (before any connections)
        {
            let mut pm = self.peer_manager.write().await;
            if let Err(e) = pm.start_streaming_listener().await {
                error!("❌ Failed to start streaming listener: {}", e);
                return Err(e);
            } else {
                info!(
                    "✅ Streaming listener started on port {}",
                    self.settings.network.port + 1
                );
            }
        }

        // Start hotkey manager
        let mut hotkey_manager = HotkeyManager::new()?;
        let peer_manager_for_hotkeys = self.peer_manager.clone();
        let clipboard_for_hotkeys = self.clipboard.clone();

        tokio::spawn(async move {
            if let Err(e) = hotkey_manager.start().await {
                error!("❌ Failed to start hotkey manager: {}", e);
                return;
            }
            info!("✅ Hotkey manager started successfully");
            Self::handle_hotkey_events(
                &mut hotkey_manager,
                peer_manager_for_hotkeys,
                clipboard_for_hotkeys,
            )
            .await;
        });

        // Start discovery service
        if let Some(discovery) = &self.discovery {
            let mut discovery_clone = discovery.clone();
            tokio::spawn(async move {
                info!("🔍 Starting discovery service...");
                if let Err(e) = discovery_clone.run().await {
                    error!("❌ Discovery service error: {}", e);
                } else {
                    info!("✅ Discovery service started successfully");
                }
            });
        }

        // ✅ FIXED: Start enhanced peer manager with intelligent routing
        let peer_manager = self.peer_manager.clone();
        let settings = self.settings.clone();
        let clipboard = self.clipboard.clone();
        tokio::spawn(async move {
            if let Err(e) =
                Self::run_intelligent_peer_manager(peer_manager, settings, clipboard).await
            {
                error!("❌ Intelligent peer manager error: {}", e);
            }
        });

        info!("✅ All enhanced background services with intelligent transfer selection started");
        Ok(())
    }

    // Keep the existing run method for non-Tauri usage (takes ownership)
    pub async fn run(mut self) -> Result<()> {
        info!("🚀 Starting Fileshare Daemon with Intelligent Transfer Selection...");
        info!("📱 Device ID: {}", self.settings.device.id);
        info!("🏷️ Device Name: {}", self.settings.device.name);
        info!("🌐 Control port: {}", self.settings.network.port);
        info!("🚀 Streaming port: {}", self.settings.network.port + 1);

        // Start hotkey manager
        info!("🎹 Initializing hotkey system...");
        if let Some(ref mut hotkey_manager) = self.hotkey_manager {
            hotkey_manager.start().await?;
            info!("✅ Hotkey manager started successfully");
        }

        // Start discovery service
        let discovery_handle = if let Some(mut discovery) = self.discovery.take() {
            tokio::spawn(async move {
                info!("🔍 Starting discovery service...");
                if let Err(e) = discovery.run().await {
                    error!("❌ Discovery service error: {}", e);
                } else {
                    info!("✅ Discovery service started successfully");
                }
            })
        } else {
            error!("❌ Discovery service not available");
            return Err(crate::FileshareError::Unknown(
                "Discovery service not available".to_string(),
            ));
        };

        // Start streaming listener
        {
            let mut pm = self.peer_manager.write().await;
            pm.start_streaming_listener().await?;
        }

        // Start peer manager
        let peer_manager_handle = {
            let peer_manager = self.peer_manager.clone();
            let settings = self.settings.clone();
            let clipboard = self.clipboard.clone();
            let mut shutdown_rx = self.shutdown_tx.subscribe();

            tokio::spawn(async move {
                tokio::select! {
                    result = Self::run_intelligent_peer_manager(peer_manager, settings, clipboard) => {
                        if let Err(e) = result {
                            error!("❌ Intelligent peer manager error: {}", e);
                        }
                    }
                    _ = shutdown_rx.recv() => {
                        info!("🛑 Peer manager shutdown requested");
                    }
                }
            })
        };

        // Start hotkey event handler
        let hotkey_handle = {
            let peer_manager = self.peer_manager.clone();
            let clipboard = self.clipboard.clone();
            let mut hotkey_manager = self.hotkey_manager.take().unwrap();
            let mut shutdown_rx = self.shutdown_tx.subscribe();

            tokio::spawn(async move {
                info!("🎹 Starting hotkey event handler...");
                tokio::select! {
                    _ = Self::handle_hotkey_events(&mut hotkey_manager, peer_manager, clipboard) => {
                        info!("🎹 Hotkey handler stopped");
                    }
                    _ = shutdown_rx.recv() => {
                        info!("🛑 Hotkey handler shutdown requested");
                        if let Err(e) = hotkey_manager.stop() {
                            error!("❌ Failed to stop hotkey manager: {}", e);
                        }
                    }
                }
            })
        };

        info!("✅ All background services with intelligent transfer selection started");

        // Wait for shutdown signal
        let mut shutdown_rx = self.shutdown_rx;
        shutdown_rx.recv().await.ok();

        // Clean shutdown
        info!("🛑 Shutting down intelligent transfer services...");
        discovery_handle.abort();
        peer_manager_handle.abort();
        hotkey_handle.abort();

        info!("✅ Fileshare Daemon with Intelligent Transfer Selection stopped");
        Ok(())
    }

    // ✅ FIXED: Simplified hotkey event handling with intelligent transfer selection
    async fn handle_hotkey_events(
        hotkey_manager: &mut HotkeyManager,
        peer_manager: Arc<RwLock<PeerManager>>,
        clipboard: ClipboardManager,
    ) {
        info!("🎹 Enhanced hotkey event handler with intelligent transfer selection active...");

        loop {
            if let Some(event) = hotkey_manager.get_event().await {
                match event {
                    HotkeyEvent::CopyFiles => {
                        info!("📋 Copy hotkey triggered - intelligent transfer method selection");
                        if let Err(e) = Self::handle_intelligent_copy_operation(
                            clipboard.clone(),
                            peer_manager.clone(),
                        )
                        .await
                        {
                            error!("❌ Failed to handle intelligent copy operation: {}", e);
                        }
                    }
                    HotkeyEvent::PasteFiles => {
                        info!("📁 Paste hotkey triggered - intelligent transfer method selection");
                        if let Err(e) = Self::handle_intelligent_paste_operation(
                            clipboard.clone(),
                            peer_manager.clone(),
                        )
                        .await
                        {
                            error!("❌ Failed to handle intelligent paste operation: {}", e);
                        }
                    }
                }
            } else {
                warn!("🎹 Hotkey event channel closed, stopping handler");
                break;
            }
        }
    }

    // ✅ FIXED: Simplified intelligent copy operation
    async fn handle_intelligent_copy_operation(
        clipboard: ClipboardManager,
        peer_manager: Arc<RwLock<PeerManager>>,
    ) -> Result<()> {
        info!("📋 Handling copy operation with intelligent transfer method selection");

        // Copy currently selected file to network clipboard
        match clipboard.copy_selected_file().await {
            Ok(()) => {
                info!("✅ File successfully copied to network clipboard");
            }
            Err(e) => {
                error!("❌ Failed to copy file: {}", e);
                Self::show_error_notification(
                    "Copy Failed",
                    &format!("Failed to copy file: {}", e),
                )
                .await?;
                return Err(e);
            }
        }

        // Get the clipboard item and determine transfer method
        let clipboard_item = {
            let clipboard_state = clipboard.network_clipboard.read().await;
            clipboard_state.clone()
        };

        if let Some(item) = clipboard_item {
            // Intelligent method selection based on file size
            let (transfer_method, icon, estimated_speed) =
                Self::determine_transfer_method_info(item.file_size);

            info!(
                "🚀 Intelligent Analysis: {:.1} MB -> {} (estimated: {})",
                item.file_size as f64 / (1024.0 * 1024.0),
                transfer_method,
                estimated_speed
            );

            // Broadcast clipboard update to healthy peers
            let healthy_peer_ids = {
                let pm = peer_manager.read().await;
                pm.get_connected_peers()
                    .iter()
                    .filter(|peer| pm.is_peer_healthy(peer.device_info.id))
                    .map(|peer| peer.device_info.id)
                    .collect::<Vec<_>>()
            };

            let peer_count = healthy_peer_ids.len();
            info!("📡 Broadcasting to {} healthy peers", peer_count);

            // Send clipboard update messages
            for peer_id in healthy_peer_ids {
                let message = crate::network::protocol::Message::new(
                    crate::network::protocol::MessageType::ClipboardUpdate {
                        file_path: item.file_path.to_string_lossy().to_string(),
                        source_device: item.source_device,
                        timestamp: item.timestamp,
                        file_size: item.file_size,
                    },
                );

                let mut pm = peer_manager.write().await;
                if let Err(e) = pm.send_message_to_peer(peer_id, message).await {
                    warn!("❌ Failed to send clipboard update to {}: {}", peer_id, e);
                }
            }

            // Show intelligent success notification
            let filename = item
                .file_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy();
            let file_size_mb = item.file_size as f64 / (1024.0 * 1024.0);

            Self::show_success_notification(
                "File Ready for Intelligent Transfer",
                &format!(
                    "{} {}\n📊 {:.1} MB\n🧠 Method: {}\n⚡ Speed: {}\n📡 {} devices ready",
                    icon, filename, file_size_mb, transfer_method, estimated_speed, peer_count
                ),
            )
            .await?;

            info!("✅ Intelligent copy operation completed successfully");
        }

        Ok(())
    }

    // ✅ FIXED: Simplified intelligent paste operation
    async fn handle_intelligent_paste_operation(
        clipboard: ClipboardManager,
        peer_manager: Arc<RwLock<PeerManager>>,
    ) -> Result<()> {
        info!("📁 Handling paste operation with intelligent transfer method selection");

        // Try to paste from network clipboard
        if let Some((target_path, source_device)) = clipboard.paste_to_current_location().await? {
            // Validate that source device is healthy
            let is_healthy = {
                let pm = peer_manager.read().await;
                pm.is_peer_healthy(source_device)
            };

            if !is_healthy {
                warn!(
                    "⚠️ Source device {} is not healthy, attempting transfer anyway",
                    source_device
                );
                Self::show_warning_notification(
                    "⚠️ Device Offline",
                    "Source device may be offline. Transfer may fail.",
                )
                .await?;
            }

            info!(
                "📁 Requesting intelligent file transfer from device {} to {:?}",
                source_device, target_path
            );

            // Get file info for method selection
            let (source_file_path, file_size) = {
                let clipboard_state = clipboard.network_clipboard.read().await;
                let item = clipboard_state.as_ref().unwrap();
                (item.file_path.to_string_lossy().to_string(), item.file_size)
            };

            // Determine transfer method and show info
            let (transfer_method, icon, estimated_speed) =
                Self::determine_transfer_method_info(file_size);

            info!(
                "🚀 Intelligent Selection: {} for {:.1} MB file (estimated: {})",
                transfer_method,
                file_size as f64 / (1024.0 * 1024.0),
                estimated_speed
            );

            // Send file request to source device
            let request_id = uuid::Uuid::new_v4();
            let message = crate::network::protocol::Message::new(
                crate::network::protocol::MessageType::FileRequest {
                    request_id,
                    file_path: source_file_path,
                    target_path: target_path.to_string_lossy().to_string(),
                },
            );

            // Send the message
            let send_result = {
                let mut pm = peer_manager.write().await;
                pm.send_message_to_peer(source_device, message).await
            };

            match send_result {
                Ok(()) => {
                    let file_size_mb = file_size as f64 / (1024.0 * 1024.0);

                    Self::show_info_notification(
                        "🧠 Intelligent Transfer Starting",
                        &format!(
                            "{} Method: {}\n📊 Size: {:.1} MB\n⚡ Speed: {}\n📂 From: {}\n📥 To: {}",
                            icon,
                            transfer_method,
                            file_size_mb,
                            estimated_speed,
                            source_device.to_string().chars().take(8).collect::<String>(),
                            target_path.file_name().unwrap_or_default().to_string_lossy()
                        ),
                    )
                    .await?;

                    info!("✅ Intelligent file request sent to source device");
                }
                Err(e) => {
                    error!("❌ Failed to send file request: {}", e);
                    Self::show_error_notification(
                        "❌ Transfer Failed",
                        &format!("Could not contact source device: {}", e),
                    )
                    .await?;
                    return Err(e);
                }
            }
        }

        Ok(())
    }

    // ✅ FIXED: Simplified transfer method determination
    fn determine_transfer_method_info(file_size: u64) -> (String, String, String) {
        if file_size >= 50 * 1024 * 1024 {
            (
                "High-Speed Streaming".to_string(),
                "🚀".to_string(),
                "50+ MB/s".to_string(),
            )
        } else {
            (
                "Standard Chunked".to_string(),
                "📦".to_string(),
                "10-20 MB/s".to_string(),
            )
        }
    }

    // ✅ FIXED: Simplified intelligent peer manager with clean message routing
    async fn run_intelligent_peer_manager(
        peer_manager: Arc<RwLock<PeerManager>>,
        settings: Arc<Settings>,
        clipboard: ClipboardManager,
    ) -> Result<()> {
        let listener = tokio::net::TcpListener::bind(settings.get_bind_address()).await?;
        info!(
            "🌐 Intelligent peer manager listening on {}",
            settings.get_bind_address()
        );

        // ✅ FIXED: Simplified connection monitoring
        let health_pm = peer_manager.clone();
        let health_monitor = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            loop {
                interval.tick().await;

                let mut pm = health_pm.write().await;
                if let Err(e) = pm.check_peer_health_all().await {
                    error!("❌ Health monitoring error: {}", e);
                }

                let stats = pm.get_connection_stats();
                if stats.total > 0 {
                    info!(
                        "📊 Connection Stats: {} total, {} healthy",
                        stats.total, stats.authenticated
                    );
                }
            }
        });

        // ✅ FIXED: Simplified transfer monitoring
        let transfer_pm = peer_manager.clone();
        let transfer_monitor = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(15));
            loop {
                interval.tick().await;

                let pm = transfer_pm.read().await;
                let mut ft = pm.file_transfer.write().await;

                // Monitor both chunked and streaming transfers
                if let Err(e) = ft.monitor_transfer_health().await {
                    error!("❌ Transfer health monitoring error: {}", e);
                }

                // Enhanced cleanup
                ft.cleanup_stale_transfers_enhanced();
            }
        });

        // ✅ FIXED: Simplified connection handler
        let connection_pm = peer_manager.clone();
        let connection_handle = tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, addr)) => {
                        info!("🔗 New connection from {}", addr);
                        let pm = connection_pm.clone();

                        tokio::spawn(async move {
                            let mut pm = pm.write().await;
                            if let Err(e) = pm.handle_connection(stream).await {
                                warn!("❌ Failed to handle connection from {}: {}", addr, e);
                            }
                        });
                    }
                    Err(e) => {
                        error!("❌ Failed to accept connection: {}", e);
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        });

        // ✅ FIXED: Clean and efficient message processing
        let message_pm = peer_manager.clone();
        let message_clipboard = clipboard.clone();
        let message_handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(50));
            loop {
                interval.tick().await;

                let mut pm = message_pm.write().await;

                // Process all pending messages
                while let Ok((peer_id, message)) = pm.message_rx.try_recv() {
                    // Basic health check
                    if !pm.is_peer_healthy(peer_id) {
                        warn!("⚠️ Dropping message from unhealthy peer {}", peer_id);
                        continue;
                    }

                    // ✅ FIXED: Simple direct message processing - let FileTransferManager handle routing
                    if let Err(e) = pm
                        .handle_message(peer_id, message, &message_clipboard)
                        .await
                    {
                        error!("❌ Error processing message from peer {}: {}", peer_id, e);
                    }
                }
            }
        });

        // Wait for any service to complete
        tokio::select! {
            _ = connection_handle => info!("Connection handler stopped"),
            _ = message_handle => info!("Message handler stopped"),
            _ = health_monitor => info!("Health monitor stopped"),
            _ = transfer_monitor => info!("Transfer monitor stopped"),
        }

        Ok(())
    }

    // ✅ FIXED: Simplified notification helpers
    async fn show_success_notification(title: &str, message: &str) -> Result<()> {
        notify_rust::Notification::new()
            .summary(title)
            .body(message)
            .timeout(notify_rust::Timeout::Milliseconds(4000))
            .show()
            .map_err(|e| crate::FileshareError::Unknown(format!("Notification error: {}", e)))?;
        Ok(())
    }

    async fn show_info_notification(title: &str, message: &str) -> Result<()> {
        notify_rust::Notification::new()
            .summary(title)
            .body(message)
            .timeout(notify_rust::Timeout::Milliseconds(5000))
            .show()
            .map_err(|e| crate::FileshareError::Unknown(format!("Notification error: {}", e)))?;
        Ok(())
    }

    async fn show_warning_notification(title: &str, message: &str) -> Result<()> {
        notify_rust::Notification::new()
            .summary(title)
            .body(message)
            .timeout(notify_rust::Timeout::Milliseconds(3000))
            .show()
            .map_err(|e| crate::FileshareError::Unknown(format!("Notification error: {}", e)))?;
        Ok(())
    }

    async fn show_error_notification(title: &str, message: &str) -> Result<()> {
        notify_rust::Notification::new()
            .summary(title)
            .body(message)
            .timeout(notify_rust::Timeout::Milliseconds(5000))
            .show()
            .map_err(|e| crate::FileshareError::Unknown(format!("Notification error: {}", e)))?;
        Ok(())
    }
}
