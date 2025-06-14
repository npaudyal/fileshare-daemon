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
use tracing::{debug, error, info, warn};

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

        // ✅ FIXED: Initialize clipboard manager with broadcast capability
        let mut clipboard = ClipboardManager::new(settings.device.id);

        // Create clipboard broadcast channel
        let (clipboard_tx, mut clipboard_rx) = tokio::sync::mpsc::unbounded_channel();
        clipboard.set_broadcast_sender(clipboard_tx);

        // Start clipboard broadcast handler
        let clipboard_peer_manager = peer_manager.clone();
        tokio::spawn(async move {
            while let Some(clipboard_item) = clipboard_rx.recv().await {
                info!(
                    "📡 DAEMON: Received clipboard broadcast request for file: {:?}",
                    clipboard_item.file_path
                );

                // Get healthy peers and broadcast
                let healthy_peers = {
                    let pm = clipboard_peer_manager.read().await;
                    pm.get_connected_peers()
                        .iter()
                        .filter(|peer| pm.is_peer_healthy(peer.device_info.id))
                        .map(|peer| peer.device_info.id)
                        .collect::<Vec<_>>()
                };

                info!(
                    "📡 DAEMON: Broadcasting to {} healthy peers",
                    healthy_peers.len()
                );

                for peer_id in healthy_peers {
                    let message = crate::network::protocol::Message::new(
                        crate::network::protocol::MessageType::ClipboardUpdate {
                            file_path: clipboard_item.file_path.to_string_lossy().to_string(),
                            source_device: clipboard_item.source_device,
                            timestamp: clipboard_item.timestamp,
                            file_size: clipboard_item.file_size,
                        },
                    );

                    let mut pm = clipboard_peer_manager.write().await;
                    if let Err(e) = pm.send_message_to_peer(peer_id, message).await {
                        error!(
                            "❌ DAEMON: Failed to send clipboard update to peer {}: {}",
                            peer_id, e
                        );
                    } else {
                        info!("✅ DAEMON: Sent clipboard update to peer {}", peer_id);
                    }
                }
            }
        });

        info!("🚀 FileshareDaemon initialized with intelligent transfer selection");
        info!("📊 Transfer Logic: <50MB = Chunked Protocol, ≥50MB = Streaming Protocol");
        info!(
            "📋 Clipboard Manager initialized with broadcast capability for device: {}",
            settings.device.id
        );

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

    // ✅ ENHANCED: Daemon startup with proper streaming integration and enhanced logging
    pub async fn start_background_services(self: Arc<Self>) -> Result<()> {
        info!("🚀 Starting Enhanced Fileshare Daemon with Intelligent Transfer Selection...");
        info!("📱 Device ID: {}", self.settings.device.id);
        info!("🏷️ Device Name: {}", self.settings.device.name);
        info!("🌐 Control port: {}", self.settings.network.port);
        info!("🚀 Streaming port: {}", self.settings.network.port + 1);
        info!("🧠 Smart Selection: Auto-chooses chunked vs streaming based on file size");

        // ✅ ENHANCED: Start streaming listener FIRST with better error handling
        {
            let mut pm = self.peer_manager.write().await;
            match pm.start_streaming_listener().await {
                Ok(()) => {
                    info!(
                        "✅ Streaming listener started on port {}",
                        self.settings.network.port + 1
                    );
                }
                Err(e) => {
                    error!("❌ Failed to start streaming listener: {}", e);
                    return Err(e);
                }
            }
        }

        // Start hotkey manager with enhanced error handling
        let mut hotkey_manager = HotkeyManager::new()?;
        let peer_manager_for_hotkeys = self.peer_manager.clone();
        let clipboard_for_hotkeys = self.clipboard.clone();

        tokio::spawn(async move {
            match hotkey_manager.start().await {
                Ok(()) => {
                    info!("✅ Hotkey manager started successfully");
                    Self::handle_hotkey_events(
                        &mut hotkey_manager,
                        peer_manager_for_hotkeys,
                        clipboard_for_hotkeys,
                    )
                    .await;
                }
                Err(e) => {
                    error!("❌ Failed to start hotkey manager: {}", e);
                }
            }
        });

        // Start discovery service
        if let Some(discovery) = &self.discovery {
            let mut discovery_clone = discovery.clone();
            tokio::spawn(async move {
                info!("🔍 Starting discovery service...");
                match discovery_clone.run().await {
                    Ok(()) => {
                        info!("✅ Discovery service started successfully");
                    }
                    Err(e) => {
                        error!("❌ Discovery service error: {}", e);
                    }
                }
            });
        }

        // ✅ ENHANCED: Start intelligent peer manager with comprehensive error handling
        let peer_manager = self.peer_manager.clone();
        let settings = self.settings.clone();
        let clipboard = self.clipboard.clone();
        tokio::spawn(async move {
            match Self::run_intelligent_peer_manager(peer_manager, settings, clipboard).await {
                Ok(()) => {
                    info!("✅ Intelligent peer manager completed successfully");
                }
                Err(e) => {
                    error!("❌ Intelligent peer manager error: {}", e);
                }
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

    // ✅ ENHANCED: Hotkey event handling with comprehensive logging and error handling
    async fn handle_hotkey_events(
        hotkey_manager: &mut HotkeyManager,
        peer_manager: Arc<RwLock<PeerManager>>,
        clipboard: ClipboardManager,
    ) {
        info!("🎹 Enhanced hotkey event handler with intelligent transfer selection active...");

        let mut event_count = 0u64;

        loop {
            if let Some(event) = hotkey_manager.get_event().await {
                event_count += 1;
                info!("🎹 Received hotkey event #{}: {:?}", event_count, event);

                match event {
                    HotkeyEvent::CopyFiles => {
                        info!("📋 Copy hotkey triggered - intelligent transfer method selection (event #{})", event_count);
                        match Self::handle_intelligent_copy_operation(
                            clipboard.clone(),
                            peer_manager.clone(),
                        )
                        .await
                        {
                            Ok(()) => {
                                info!("✅ Copy operation #{} completed successfully", event_count);
                            }
                            Err(e) => {
                                error!("❌ Copy operation #{} failed: {}", event_count, e);
                            }
                        }
                    }
                    HotkeyEvent::PasteFiles => {
                        info!("📁 Paste hotkey triggered - intelligent transfer method selection (event #{})", event_count);
                        match Self::handle_intelligent_paste_operation(
                            clipboard.clone(),
                            peer_manager.clone(),
                        )
                        .await
                        {
                            Ok(()) => {
                                info!("✅ Paste operation #{} completed successfully", event_count);
                            }
                            Err(e) => {
                                error!("❌ Paste operation #{} failed: {}", event_count, e);
                            }
                        }
                    }
                }
            } else {
                warn!("🎹 Hotkey event channel closed, stopping handler");
                break;
            }
        }

        info!(
            "🎹 Hotkey event handler stopped after processing {} events",
            event_count
        );
    }

    // ✅ ENHANCED: Intelligent copy operation with comprehensive error handling and state tracking
    async fn handle_intelligent_copy_operation(
        clipboard: ClipboardManager,
        _peer_manager: Arc<RwLock<PeerManager>>, // No longer needed for broadcasting
    ) -> Result<()> {
        info!("📋 COPY_OP: Starting intelligent copy operation");

        // The clipboard manager now handles everything including broadcasting!
        match clipboard.copy_selected_file().await {
            Ok(()) => {
                info!("✅ COPY_OP: Copy operation with broadcasting completed successfully");
            }
            Err(e) => {
                error!("❌ COPY_OP: Copy operation failed: {}", e);
                Self::show_error_notification(
                    "Copy Failed",
                    &format!("Failed to copy file: {}", e),
                )
                .await?;
                return Err(e);
            }
        }

        Ok(())
    }

    // ✅ ENHANCED: Intelligent paste operation with comprehensive error handling and state tracking
    async fn handle_intelligent_paste_operation(
        clipboard: ClipboardManager,
        peer_manager: Arc<RwLock<PeerManager>>,
    ) -> Result<()> {
        info!("📁 PASTE_OP: Starting intelligent paste operation");

        // Debug clipboard state before operation
        clipboard.debug_clipboard_state().await;

        // Try to paste from network clipboard
        let paste_result = clipboard.paste_to_current_location().await;

        match paste_result {
            Ok(Some((target_path, source_device))) => {
                info!(
                    "📁 PASTE_OP: Paste target: {:?}, source device: {}",
                    target_path, source_device
                );

                // Validate that source device is healthy and get peer info
                let (is_healthy, peer_info) = {
                    let pm = peer_manager.read().await;
                    let healthy = pm.is_peer_healthy(source_device);
                    let peer = pm.peers.get(&source_device).cloned();
                    (healthy, peer)
                };

                if !is_healthy {
                    warn!(
                        "⚠️ PASTE_OP: Source device {} is not healthy, attempting transfer anyway",
                        source_device
                    );

                    // Show warning but continue
                    Self::show_warning_notification(
                        "⚠️ Device Offline",
                        "Source device may be offline. Transfer may fail.",
                    )
                    .await?;
                }

                // Get file info for the paste request
                let (source_file_path, file_size) = {
                    let clipboard_state = clipboard.network_clipboard.read().await;
                    match clipboard_state.as_ref() {
                        Some(item) => {
                            (item.file_path.to_string_lossy().to_string(), item.file_size)
                        }
                        None => {
                            error!("❌ PASTE_OP: Clipboard is empty during paste - race condition detected");
                            return Err(crate::FileshareError::Unknown(
                                "Clipboard became empty during paste operation".to_string(),
                            ));
                        }
                    }
                };

                info!(
                    "📁 PASTE_OP: Requesting file: {} ({:.1}MB) from device {}",
                    source_file_path,
                    file_size as f64 / (1024.0 * 1024.0),
                    source_device
                );

                // Determine transfer method and show info
                let (transfer_method, icon, estimated_speed) =
                    Self::determine_transfer_method_info(file_size);

                // Send file request to source device
                let request_id = uuid::Uuid::new_v4();
                let message = crate::network::protocol::Message::new(
                    crate::network::protocol::MessageType::FileRequest {
                        request_id,
                        file_path: source_file_path,
                        target_path: target_path.to_string_lossy().to_string(),
                    },
                );

                info!(
                    "📁 PASTE_OP: Sending FileRequest {} to device {}",
                    request_id, source_device
                );

                // Send the message with comprehensive error handling
                let send_result = {
                    let mut pm = peer_manager.write().await;
                    pm.send_message_to_peer(source_device, message).await
                };

                match send_result {
                    Ok(()) => {
                        let file_size_mb = file_size as f64 / (1024.0 * 1024.0);

                        Self::show_info_notification(
                            "🧠 Transfer Request Sent",
                            &format!(
                                "{} Method: {}\n📊 Size: {:.1} MB\n⚡ Speed: {}\n📂 From: {}\n📥 To: {}",
                                icon,
                                transfer_method,
                                file_size_mb,
                                estimated_speed,
                                peer_info.map(|p| p.device_info.name).unwrap_or_else(|| "Unknown Device".to_string()),
                                target_path.file_name().unwrap_or_default().to_string_lossy()
                            ),
                        ).await?;

                        info!(
                            "✅ PASTE_OP: File request sent successfully to device {}",
                            source_device
                        );
                    }
                    Err(e) => {
                        error!(
                            "❌ PASTE_OP: Failed to send file request to device {}: {}",
                            source_device, e
                        );
                        Self::show_error_notification(
                            "❌ Transfer Request Failed",
                            &format!("Could not contact source device: {}", e),
                        )
                        .await?;
                        return Err(e);
                    }
                }
            }
            Ok(None) => {
                info!("📁 PASTE_OP: No paste operation needed (same device or other reason)");
                // This is normal - could be same device or clipboard empty
            }
            Err(e) => {
                error!("❌ PASTE_OP: Paste operation failed: {}", e);
                Self::show_error_notification(
                    "❌ Paste Failed",
                    &format!("Paste operation failed: {}", e),
                )
                .await?;
                return Err(e);
            }
        }

        Ok(())
    }

    // ✅ ENHANCED: Transfer method determination with detailed info
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

    // ✅ ENHANCED: Intelligent peer manager with comprehensive message processing and error handling
    async fn run_intelligent_peer_manager(
        peer_manager: Arc<RwLock<PeerManager>>,
        settings: Arc<Settings>,
        clipboard: ClipboardManager,
    ) -> Result<()> {
        let listener = tokio::net::TcpListener::bind(settings.get_bind_address()).await?;
        info!(
            "🌐 Enhanced peer manager listening on {}",
            settings.get_bind_address()
        );

        // ✅ Enhanced connection monitoring with better logging
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
                        "📊 Health Check: {} total connections, {} authenticated, {} unhealthy",
                        stats.total, stats.authenticated, stats.unhealthy
                    );
                } else {
                    debug!("📊 Health Check: No active connections");
                }
            }
        });

        // ✅ Enhanced transfer monitoring
        let transfer_pm = peer_manager.clone();
        let transfer_monitor = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(15));
            loop {
                interval.tick().await;

                let pm = transfer_pm.read().await;
                let mut ft = pm.file_transfer.write().await;

                if let Err(e) = ft.monitor_transfer_health().await {
                    error!("❌ Transfer health monitoring error: {}", e);
                }

                ft.cleanup_stale_transfers_enhanced();
            }
        });

        // ✅ Enhanced connection handler
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
                            } else {
                                info!("✅ Successfully handled connection from {}", addr);
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

        // ✅ CRITICAL FIX: Enhanced message processing with proper error handling and clipboard debugging
        let message_pm = peer_manager.clone();
        let message_clipboard = clipboard.clone();
        let message_handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(50));
            let mut message_count = 0u64;
            let mut clipboard_message_count = 0u64;
            let mut last_stats_log = tokio::time::Instant::now();

            loop {
                interval.tick().await;

                let mut pm = message_pm.write().await;

                // Process all pending messages with comprehensive error handling
                let mut processed_count = 0;
                let mut error_count = 0;
                let mut clipboard_updates = 0;

                while let Ok((peer_id, message)) = pm.message_rx.try_recv() {
                    processed_count += 1;
                    message_count += 1;

                    // Track clipboard messages specifically
                    match &message.message_type {
                        crate::network::protocol::MessageType::ClipboardUpdate { .. } => {
                            clipboard_message_count += 1;
                            clipboard_updates += 1;
                            info!(
                                "📋 Processing ClipboardUpdate #{} from peer {}",
                                clipboard_message_count, peer_id
                            );
                        }
                        crate::network::protocol::MessageType::FileRequest { .. } => {
                            info!("📁 Processing FileRequest from peer {}", peer_id);
                        }
                        _ => {
                            debug!(
                                "📥 Processing message #{} from peer {}: {:?}",
                                message_count, peer_id, message.message_type
                            );
                        }
                    }

                    // Health check with detailed logging
                    if !pm.is_peer_healthy(peer_id) {
                        warn!(
                            "⚠️ Dropping message from unhealthy peer {}: {:?}",
                            peer_id, message.message_type
                        );
                        continue;
                    }

                    // Process message with detailed error handling
                    match pm
                        .handle_message(peer_id, message.clone(), &message_clipboard)
                        .await
                    {
                        Ok(()) => match &message.message_type {
                            crate::network::protocol::MessageType::ClipboardUpdate { .. } => {
                                info!(
                                    "✅ Successfully processed ClipboardUpdate #{} from peer {}",
                                    clipboard_message_count, peer_id
                                );
                            }
                            _ => {
                                debug!(
                                    "✅ Successfully processed message #{} from peer {}",
                                    message_count, peer_id
                                );
                            }
                        },
                        Err(e) => {
                            error_count += 1;
                            error!(
                                "❌ Error #{} processing message from peer {}: {} | Message: {:?}",
                                error_count, peer_id, e, message.message_type
                            );

                            // Add specific error handling for clipboard messages
                            match message.message_type {
                                crate::network::protocol::MessageType::ClipboardUpdate {
                                    ..
                                } => {
                                    error!("❌ CRITICAL: ClipboardUpdate message failed - clipboard sync may be broken");
                                    // Debug clipboard state after failed update
                                    message_clipboard.debug_clipboard_state().await;
                                }
                                crate::network::protocol::MessageType::FileRequest { .. } => {
                                    error!("❌ CRITICAL: FileRequest message failed - paste operation may fail");
                                }
                                _ => {}
                            }
                        }
                    }
                }

                // Periodic stats logging with clipboard-specific info
                if processed_count > 0 {
                    if clipboard_updates > 0 {
                        info!("📊 Message batch: processed {} messages ({} clipboard updates), {} errors", 
                              processed_count, clipboard_updates, error_count);
                    } else {
                        debug!(
                            "📊 Message batch: processed {} messages, {} errors",
                            processed_count, error_count
                        );
                    }
                }

                if last_stats_log.elapsed() >= Duration::from_secs(60) {
                    info!(
                        "📊 Message Stats: {} total messages, {} clipboard messages processed",
                        message_count, clipboard_message_count
                    );
                    last_stats_log = tokio::time::Instant::now();
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

    // ✅ ENHANCED: Notification helpers with better error handling
    async fn show_success_notification(title: &str, message: &str) -> Result<()> {
        match notify_rust::Notification::new()
            .summary(title)
            .body(message)
            .timeout(notify_rust::Timeout::Milliseconds(4000))
            .show()
        {
            Ok(_) => {
                debug!("✅ Shown success notification: {}", title);
                Ok(())
            }
            Err(e) => {
                warn!("⚠️ Failed to show success notification '{}': {}", title, e);
                Err(crate::FileshareError::Unknown(format!(
                    "Notification error: {}",
                    e
                )))
            }
        }
    }

    async fn show_info_notification(title: &str, message: &str) -> Result<()> {
        match notify_rust::Notification::new()
            .summary(title)
            .body(message)
            .timeout(notify_rust::Timeout::Milliseconds(5000))
            .show()
        {
            Ok(_) => {
                debug!("✅ Shown info notification: {}", title);
                Ok(())
            }
            Err(e) => {
                warn!("⚠️ Failed to show info notification '{}': {}", title, e);
                Err(crate::FileshareError::Unknown(format!(
                    "Notification error: {}",
                    e
                )))
            }
        }
    }

    async fn show_warning_notification(title: &str, message: &str) -> Result<()> {
        match notify_rust::Notification::new()
            .summary(title)
            .body(message)
            .timeout(notify_rust::Timeout::Milliseconds(3000))
            .show()
        {
            Ok(_) => {
                debug!("✅ Shown warning notification: {}", title);
                Ok(())
            }
            Err(e) => {
                warn!("⚠️ Failed to show warning notification '{}': {}", title, e);
                Err(crate::FileshareError::Unknown(format!(
                    "Notification error: {}",
                    e
                )))
            }
        }
    }

    async fn show_error_notification(title: &str, message: &str) -> Result<()> {
        match notify_rust::Notification::new()
            .summary(title)
            .body(message)
            .timeout(notify_rust::Timeout::Milliseconds(5000))
            .show()
        {
            Ok(_) => {
                debug!("✅ Shown error notification: {}", title);
                Ok(())
            }
            Err(e) => {
                error!("❌ Failed to show error notification '{}': {}", title, e);
                Err(crate::FileshareError::Unknown(format!(
                    "Notification error: {}",
                    e
                )))
            }
        }
    }

    // ✅ NEW: Public method to get clipboard state for debugging
    pub async fn get_clipboard_debug_info(&self) -> String {
        let clipboard_state = self.clipboard.network_clipboard.read().await;
        match clipboard_state.as_ref() {
            Some(item) => {
                format!(
                    "📋 Clipboard contains:\n\
                    File: {:?}\n\
                    Source Device: {}\n\
                    Size: {:.1} MB\n\
                    Timestamp: {}",
                    item.file_path,
                    item.source_device,
                    item.file_size as f64 / (1024.0 * 1024.0),
                    item.timestamp
                )
            }
            None => "📋 Clipboard is empty".to_string(),
        }
    }

    // ✅ NEW: Force clipboard debug
    pub async fn debug_clipboard_state(&self) {
        self.clipboard.debug_clipboard_state().await;
    }
}
