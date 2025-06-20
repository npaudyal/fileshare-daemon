use crate::{
    clipboard::ClipboardManager,
    config::Settings,
    hotkeys::{HotkeyEvent, HotkeyManager},
    network::{DiscoveryService, PeerManager},
    service::file_transfer::TransferDirection,
    utils::format_file_size,
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

        // Initialize peer manager
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

    // Enhanced daemon startup with streaming support
    pub async fn start_background_services(mut self: Arc<Self>) -> Result<()> {
        info!("🚀 Starting Enhanced Fileshare Daemon with streaming support...");
        info!("📱 Device ID: {}", self.settings.device.id);
        info!("🏷️ Device Name: {}", self.settings.device.name);
        info!("🌐 Listening on port: {}", self.settings.network.port);
        info!(
            "📊 Streaming enabled: {}",
            self.settings.streaming.enable_streaming_mode
        );
        info!(
            "📦 Max memory buffer: {} MB",
            self.settings.streaming.max_memory_buffer_mb
        );

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

        // Start peer manager with streaming support
        let peer_manager = self.peer_manager.clone();
        let settings = self.settings.clone();
        let clipboard = self.clipboard.clone();
        tokio::spawn(async move {
            if let Err(e) =
                Self::run_enhanced_peer_manager_with_streaming(peer_manager, settings, clipboard)
                    .await
            {
                error!("❌ Enhanced peer manager error: {}", e);
            }
        });

        info!("✅ All enhanced background services with streaming started successfully");
        Ok(())
    }

    // Keep the existing run method for non-Tauri usage (takes ownership)
    pub async fn run(mut self) -> Result<()> {
        info!("🚀 Starting Fileshare Daemon with streaming...");
        info!("📱 Device ID: {}", self.settings.device.id);
        info!("🏷️ Device Name: {}", self.settings.device.name);
        info!("🌐 Listening on port: {}", self.settings.network.port);

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

        // Start peer manager with streaming
        let peer_manager_handle = {
            let peer_manager = self.peer_manager.clone();
            let settings = self.settings.clone();
            let clipboard = self.clipboard.clone();
            let mut shutdown_rx = self.shutdown_tx.subscribe();

            tokio::spawn(async move {
                tokio::select! {
                    result = Self::run_enhanced_peer_manager_with_streaming(peer_manager, settings, clipboard) => {
                        if let Err(e) = result {
                            error!("❌ Peer manager error: {}", e);
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

        info!("✅ All background services started successfully");

        // Wait for shutdown signal
        let mut shutdown_rx = self.shutdown_rx;
        shutdown_rx.recv().await.ok();

        // Clean shutdown
        info!("🛑 Shutting down services...");
        discovery_handle.abort();
        peer_manager_handle.abort();
        hotkey_handle.abort();

        info!("✅ Fileshare Daemon stopped");
        Ok(())
    }

    async fn handle_hotkey_events(
        hotkey_manager: &mut HotkeyManager,
        peer_manager: Arc<RwLock<PeerManager>>,
        clipboard: ClipboardManager,
    ) {
        info!("🎹 Enhanced hotkey event handler active with streaming support...");

        loop {
            if let Some(event) = hotkey_manager.get_event().await {
                match event {
                    HotkeyEvent::CopyFiles => {
                        info!(
                            "📋 Copy hotkey triggered - copying selected file with streaming support"
                        );
                        if let Err(e) = Self::handle_copy_operation_streaming(
                            clipboard.clone(),
                            peer_manager.clone(),
                        )
                        .await
                        {
                            error!("❌ Failed to handle streaming copy operation: {}", e);
                        }
                    }
                    HotkeyEvent::PasteFiles => {
                        info!("📁 Paste hotkey triggered - pasting with streaming support");
                        if let Err(e) = Self::handle_paste_operation_streaming(
                            clipboard.clone(),
                            peer_manager.clone(),
                        )
                        .await
                        {
                            error!("❌ Failed to handle streaming paste operation: {}", e);
                        }
                    }
                }
            } else {
                warn!("🎹 Hotkey event channel closed, stopping handler");
                break;
            }
        }
    }

    // Enhanced copy operation with streaming support - NO SIZE LIMITS!
    async fn handle_copy_operation_streaming(
        clipboard: ClipboardManager,
        peer_manager: Arc<RwLock<PeerManager>>,
    ) -> Result<()> {
        info!("📋 Handling streaming copy operation with unlimited file size support");

        // Copy currently selected file to network clipboard
        match clipboard.copy_selected_file().await {
            Ok(()) => {
                info!("✅ File successfully copied to network clipboard");
            }
            Err(e) => {
                error!("❌ Failed to copy file: {}", e);

                // Show error notification
                notify_rust::Notification::new()
                    .summary("Copy Failed")
                    .body(&format!("Failed to copy file: {}", e))
                    .timeout(notify_rust::Timeout::Milliseconds(5000))
                    .show()
                    .map_err(|e| {
                        crate::FileshareError::Unknown(format!("Notification error: {}", e))
                    })?;

                return Err(e);
            }
        }

        // Get the clipboard item to broadcast to other devices
        let clipboard_item = {
            let clipboard_state = clipboard.network_clipboard.read().await;
            clipboard_state.clone()
        };

        if let Some(item) = clipboard_item {
            // Show file size info - NO LIMITS!
            let file_size_str = if item.file_size < 1024 {
                format!("{} B", item.file_size)
            } else if item.file_size < 1024 * 1024 {
                format!("{:.1} KB", item.file_size as f64 / 1024.0)
            } else if item.file_size < 1024 * 1024 * 1024 {
                format!("{:.1} MB", item.file_size as f64 / (1024.0 * 1024.0))
            } else if item.file_size < 1024 * 1024 * 1024 * 1024 {
                format!(
                    "{:.1} GB",
                    item.file_size as f64 / (1024.0 * 1024.0 * 1024.0)
                )
            } else {
                format!(
                    "{:.1} TB",
                    item.file_size as f64 / (1024.0 * 1024.0 * 1024.0 * 1024.0)
                )
            };

            info!("📊 File size: {} ({})", item.file_size, file_size_str);

            // Broadcast clipboard update to healthy peers only
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

            // Send messages to each healthy peer
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

            // Show enhanced success notification
            let filename = item
                .file_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy();

            notify_rust::Notification::new()
                .summary("🚀 File Copied to Network (Streaming)")
                .body(&format!(
                    "✅ {}\n📦 Size: {}\n📡 Shared with {} devices\n⚡ Streaming: Enabled",
                    filename, file_size_str, peer_count
                ))
                .timeout(notify_rust::Timeout::Milliseconds(4000))
                .show()
                .map_err(|e| {
                    crate::FileshareError::Unknown(format!("Notification error: {}", e))
                })?;

            info!("✅ Enhanced streaming copy operation completed successfully");
        }

        Ok(())
    }

    async fn handle_paste_operation_streaming(
        clipboard: ClipboardManager,
        peer_manager: Arc<RwLock<PeerManager>>,
    ) -> Result<()> {
        info!("📁 Handling streaming paste operation with unlimited file size support");

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

                notify_rust::Notification::new()
                    .summary("⚠️ Device Offline")
                    .body("Source device may be offline. Transfer may fail.")
                    .timeout(notify_rust::Timeout::Milliseconds(3000))
                    .show()
                    .map_err(|e| {
                        crate::FileshareError::Unknown(format!("Notification error: {}", e))
                    })?;
            }

            info!(
                "📁 Requesting streaming file transfer from device {} to {:?}",
                source_device, target_path
            );

            // Get the source file path and file size
            let (source_file_path, file_size) = {
                let clipboard_state = clipboard.network_clipboard.read().await;
                let item = clipboard_state.as_ref().unwrap();
                (item.file_path.to_string_lossy().to_string(), item.file_size)
            };

            // Show file size - NO LIMITS with streaming!
            let file_size_str = if file_size < 1024 {
                format!("{} B", file_size)
            } else if file_size < 1024 * 1024 {
                format!("{:.1} KB", file_size as f64 / 1024.0)
            } else if file_size < 1024 * 1024 * 1024 {
                format!("{:.1} MB", file_size as f64 / (1024.0 * 1024.0))
            } else if file_size < 1024 * 1024 * 1024 * 1024 {
                format!("{:.1} GB", file_size as f64 / (1024.0 * 1024.0 * 1024.0))
            } else {
                format!(
                    "{:.1} TB",
                    file_size as f64 / (1024.0 * 1024.0 * 1024.0 * 1024.0)
                )
            };

            info!("📊 File size: {} ({})", file_size, file_size_str);

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
                    // Show enhanced notification with file details
                    notify_rust::Notification::new()
                        .summary("🚀 Streaming Transfer Starting")
                        .body(&format!(
                            "Requesting file ({})\nFrom: {}\nTo: {}\n⚡ Mode: Streaming",
                            file_size_str,
                            source_device
                                .to_string()
                                .chars()
                                .take(8)
                                .collect::<String>(),
                            target_path
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                        ))
                        .timeout(notify_rust::Timeout::Milliseconds(5000))
                        .show()
                        .map_err(|e| {
                            crate::FileshareError::Unknown(format!("Notification error: {}", e))
                        })?;

                    info!("✅ Enhanced streaming file request sent to source device");
                }
                Err(e) => {
                    error!("❌ Failed to send file request: {}", e);

                    notify_rust::Notification::new()
                        .summary("❌ Transfer Failed")
                        .body(&format!("Could not contact source device: {}", e))
                        .timeout(notify_rust::Timeout::Milliseconds(5000))
                        .show()
                        .map_err(|e| {
                            crate::FileshareError::Unknown(format!("Notification error: {}", e))
                        })?;

                    return Err(e);
                }
            }
        }

        Ok(())
    }

    // Enhanced peer manager with streaming support
    async fn run_enhanced_peer_manager_with_streaming(
        peer_manager: Arc<RwLock<PeerManager>>,
        settings: Arc<Settings>,
        clipboard: ClipboardManager,
    ) -> Result<()> {
        let listener = tokio::net::TcpListener::bind(settings.get_bind_address()).await?;
        info!(
            "🌐 Enhanced peer manager with streaming listening on {}",
            settings.get_bind_address()
        );

        info!("📊 Streaming configuration:");
        info!(
            "  • Streaming enabled: {}",
            settings.streaming.enable_streaming_mode
        );
        info!(
            "  • Max memory buffer: {} MB",
            settings.streaming.max_memory_buffer_mb
        );
        info!(
            "  • Adaptive chunking: {}",
            settings.streaming.enable_adaptive_chunking
        );
        info!(
            "  • Chunk validation: {}",
            settings.streaming.enable_chunk_validation
        );

        // Start connection health monitoring with streaming awareness
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
                info!(
                    "📊 Connection Stats: {} total, {} healthy, {} unhealthy",
                    stats.total, stats.authenticated, stats.unhealthy
                );
            }
        });

        // Start streaming-aware file transfer health monitoring
        let transfer_pm = peer_manager.clone();
        let transfer_monitor = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(10));
            loop {
                interval.tick().await;

                let pm = transfer_pm.read().await;
                let mut ft = pm.file_transfer.write().await;

                // Monitor transfer health with streaming support
                if let Err(e) = ft.monitor_transfer_health().await {
                    error!("❌ Streaming transfer health monitoring error: {}", e);
                }

                // Enhanced cleanup for streaming transfers
                ft.cleanup_stale_transfers_enhanced();

                // Debug active transfers occasionally
                if interval.period().as_secs() % 60 == 0 {
                    ft.debug_active_transfers();
                }
            }
        });

        // Connection handler
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

        // Enhanced message processing with streaming support
        let message_pm = peer_manager.clone();
        let message_clipboard = clipboard.clone();
        let message_handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(25)); // Faster for streaming
            loop {
                interval.tick().await;

                let mut pm = message_pm.write().await;

                while let Ok((peer_id, message)) = pm.message_rx.try_recv() {
                    // Enhanced message routing with streaming health checks
                    if !pm.is_peer_healthy(peer_id) {
                        warn!("⚠️ Dropping message from unhealthy peer {}", peer_id);
                        continue;
                    }

                    // Route streaming transfer messages directly
                    match &message.message_type {
                        crate::network::protocol::MessageType::FileOffer {
                            transfer_id, ..
                        }
                        | crate::network::protocol::MessageType::FileChunk {
                            transfer_id, ..
                        }
                        | crate::network::protocol::MessageType::TransferComplete {
                            transfer_id,
                            ..
                        }
                        | crate::network::protocol::MessageType::TransferError {
                            transfer_id,
                            ..
                        }
                        | crate::network::protocol::MessageType::TransferProgress {
                            transfer_id,
                            ..
                        } => {
                            let ft = pm.file_transfer.read().await;
                            let is_outgoing = ft.has_transfer(*transfer_id)
                                && matches!(
                                    ft.get_transfer_direction(*transfer_id),
                                    Some(TransferDirection::Outgoing)
                                );
                            drop(ft);

                            if is_outgoing {
                                // Handle outgoing transfer messages
                                if matches!(
                                    message.message_type,
                                    crate::network::protocol::MessageType::TransferComplete { .. }
                                ) {
                                    info!("🚀 Processing outgoing TransferComplete for streaming transfer {} to peer {}", transfer_id, peer_id);

                                    if let Err(e) =
                                        pm.send_direct_to_connection(peer_id, message.clone()).await
                                    {
                                        error!(
                                            "❌ Failed to send TransferComplete to peer {}: {}",
                                            peer_id, e
                                        );
                                    }

                                    // Mark streaming transfer as completed
                                    {
                                        let mut ft = pm.file_transfer.write().await;
                                        if let Err(e) =
                                            ft.mark_outgoing_transfer_completed(*transfer_id).await
                                        {
                                            error!("❌ Failed to mark streaming transfer {} as completed: {}", transfer_id, e);
                                        }
                                    }

                                    continue;
                                }

                                // For all other outgoing transfer messages, send directly
                                if let Err(e) =
                                    pm.send_direct_to_connection(peer_id, message.clone()).await
                                {
                                    error!(
                                        "❌ Failed to send direct message to peer {}: {}",
                                        peer_id, e
                                    );
                                }
                                continue;
                            }
                        }
                        _ => {}
                    }

                    // Process all other messages normally
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
}
