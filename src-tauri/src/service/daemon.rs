// src/service/daemon.rs - COMPLETE UPDATE
use crate::{
    clipboard::ClipboardManager,
    config::Settings,
    hotkeys::{HotkeyEvent, HotkeyManager},
    network::{
        DiscoveryService, HighSpeedConfig, HighSpeedProgress, HighSpeedStreamingManager,
        PeerManager,
    },
    service::file_transfer::TransferDirection,
    utils::format_file_size,
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

    // NEW: High-speed streaming manager
    pub high_speed_manager: Arc<RwLock<Option<HighSpeedStreamingManager>>>,

    hotkey_manager: Option<HotkeyManager>,
    clipboard: ClipboardManager,
    shutdown_tx: broadcast::Sender<()>,
    shutdown_rx: broadcast::Receiver<()>,
}

impl FileshareDaemon {
    pub async fn new(settings: Settings) -> Result<Self> {
        let settings = Arc::new(settings);
        let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

        // Create peer manager without circular references
        let mut peer_manager = PeerManager::new(settings.clone()).await?;
        peer_manager.setup_file_transfer_callbacks().await;
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

        // Initialize clipboard manager
        let clipboard = ClipboardManager::new(settings.device.id);

        // NEW: Initialize high-speed streaming manager
        let high_speed_config = HighSpeedConfig {
            connection_count: 6,
            chunk_size: 4 * 1024 * 1024,      // 4MB chunks
            batch_size: 8 * 1024 * 1024,      // 8MB batches
            pipeline_depth: 32,               // Deep pipeline
            memory_limit: 1024 * 1024 * 1024, // 1GB memory limit
            bandwidth_target_mbps: 150.0,     // Target 150 MB/s
            ..Default::default()
        };

        let high_speed_manager = match HighSpeedStreamingManager::new(high_speed_config).await {
            Ok(manager) => {
                info!("✅ High-speed streaming manager initialized");
                Some(manager)
            }
            Err(e) => {
                warn!(
                    "⚠️ Failed to initialize high-speed streaming manager: {}",
                    e
                );
                None
            }
        };

        Ok(Self {
            settings,
            discovery: Some(discovery),
            peer_manager,
            high_speed_manager: Arc::new(RwLock::new(high_speed_manager)),
            hotkey_manager: Some(hotkey_manager),
            clipboard,
            shutdown_tx,
            shutdown_rx,
        })
    }

    // Enhanced daemon startup with high-speed streaming
    pub async fn start_background_services(mut self: Arc<Self>) -> Result<()> {
        info!("🚀 Starting Enhanced Fileshare Daemon with HIGH-SPEED streaming...");
        info!("📱 Device ID: {}", self.settings.device.id);
        info!("🏷️ Device Name: {}", self.settings.device.name);
        info!("🌐 Listening on port: {}", self.settings.network.port);

        // Start hotkey manager
        let mut hotkey_manager = HotkeyManager::new()?;
        let peer_manager_for_hotkeys = self.peer_manager.clone();
        let clipboard_for_hotkeys = self.clipboard.clone();
        let high_speed_for_hotkeys = self.high_speed_manager.clone();

        tokio::spawn(async move {
            if let Err(e) = hotkey_manager.start().await {
                error!("❌ Failed to start hotkey manager: {}", e);
                return;
            }
            info!("✅ Hotkey manager started successfully");
            Self::handle_hotkey_events_enhanced(
                &mut hotkey_manager,
                peer_manager_for_hotkeys,
                clipboard_for_hotkeys,
                high_speed_for_hotkeys,
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

        // Start enhanced peer manager with high-speed support
        let peer_manager = self.peer_manager.clone();
        let settings = self.settings.clone();
        let clipboard = self.clipboard.clone();
        let high_speed_manager = self.high_speed_manager.clone();

        tokio::spawn(async move {
            if let Err(e) = Self::run_enhanced_peer_manager_with_high_speed(
                peer_manager,
                settings,
                clipboard,
                high_speed_manager,
            )
            .await
            {
                error!("❌ Enhanced peer manager error: {}", e);
            }
        });

        info!("✅ All enhanced background services with HIGH-SPEED streaming started successfully");
        Ok(())
    }

    // Enhanced hotkey handler with high-speed transfers
    async fn handle_hotkey_events_enhanced(
        hotkey_manager: &mut HotkeyManager,
        peer_manager: Arc<RwLock<PeerManager>>,
        clipboard: ClipboardManager,
        high_speed_manager: Arc<RwLock<Option<HighSpeedStreamingManager>>>,
    ) {
        info!("🎹 Enhanced hotkey event handler with HIGH-SPEED support active...");

        loop {
            if let Some(event) = hotkey_manager.get_event().await {
                match event {
                    HotkeyEvent::CopyFiles => {
                        info!("📋 Copy hotkey triggered - HIGH-SPEED network clipboard");
                        if let Err(e) = Self::handle_copy_operation_high_speed(
                            clipboard.clone(),
                            peer_manager.clone(),
                            high_speed_manager.clone(),
                        )
                        .await
                        {
                            error!("❌ Failed to handle HIGH-SPEED copy operation: {}", e);
                        }
                    }
                    HotkeyEvent::PasteFiles => {
                        info!("📁 Paste hotkey triggered - HIGH-SPEED network paste");
                        if let Err(e) = Self::handle_paste_operation_high_speed(
                            clipboard.clone(),
                            peer_manager.clone(),
                            high_speed_manager.clone(),
                        )
                        .await
                        {
                            error!("❌ Failed to handle HIGH-SPEED paste operation: {}", e);
                        }
                    }
                }
            } else {
                warn!("🎹 Hotkey event channel closed, stopping handler");
                break;
            }
        }
    }

    // Enhanced copy operation with high-speed streaming
    async fn handle_copy_operation_high_speed(
        clipboard: ClipboardManager,
        peer_manager: Arc<RwLock<PeerManager>>,
        high_speed_manager: Arc<RwLock<Option<HighSpeedStreamingManager>>>,
    ) -> Result<()> {
        info!("📋 Handling HIGH-SPEED copy operation");

        // Copy file to network clipboard
        match clipboard.copy_selected_file().await {
            Ok(()) => {
                info!("✅ File successfully copied to HIGH-SPEED network clipboard");
            }
            Err(e) => {
                error!("❌ Failed to copy file: {}", e);

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

        // Get clipboard item to determine transfer method
        let clipboard_item = {
            let clipboard_state = clipboard.network_clipboard.read().await;
            clipboard_state.clone()
        };

        if let Some(item) = clipboard_item {
            let file_size = item.file_size;
            let transfer_method = if file_size > 500 * 1024 * 1024 {
                // 500MB threshold
                "HIGH-SPEED"
            } else if file_size > 50 * 1024 * 1024 {
                // 50MB threshold
                "STREAMING"
            } else {
                "CHUNKED"
            };

            // Broadcast clipboard update with transfer method info
            let healthy_peer_ids = {
                let pm = peer_manager.read().await;
                pm.get_connected_peers()
                    .iter()
                    .filter(|peer| pm.is_peer_healthy(peer.device_info.id))
                    .map(|peer| peer.device_info.id)
                    .collect::<Vec<_>>()
            };

            let peer_count = healthy_peer_ids.len();
            info!(
                "📡 Broadcasting to {} healthy peers using {} method",
                peer_count, transfer_method
            );

            // Send enhanced clipboard update messages
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

            // Enhanced success notification with transfer method
            let filename = item
                .file_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy();
            let file_size_mb = item.file_size as f64 / (1024.0 * 1024.0);

            notify_rust::Notification::new()
                .summary("File Copied to HIGH-SPEED Network")
                .body(&format!(
                    "🚀 {}\n📦 Size: {:.1} MB\n⚡ Method: {}\n📡 Shared with {} devices",
                    filename, file_size_mb, transfer_method, peer_count
                ))
                .timeout(notify_rust::Timeout::Milliseconds(4000))
                .show()
                .map_err(|e| {
                    crate::FileshareError::Unknown(format!("Notification error: {}", e))
                })?;

            info!("✅ HIGH-SPEED copy operation completed successfully");
        }

        Ok(())
    }

    // Enhanced paste operation with high-speed streaming
    async fn handle_paste_operation_high_speed(
        clipboard: ClipboardManager,
        peer_manager: Arc<RwLock<PeerManager>>,
        high_speed_manager: Arc<RwLock<Option<HighSpeedStreamingManager>>>,
    ) -> Result<()> {
        info!("📁 Handling HIGH-SPEED paste operation");

        if let Some((target_path, source_device)) = clipboard.paste_to_current_location().await? {
            // Validate source device health
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

            // Get file info to determine transfer method
            let (source_file_path, file_size) = {
                let clipboard_state = clipboard.network_clipboard.read().await;
                let item = clipboard_state.as_ref().unwrap();
                (item.file_path.to_string_lossy().to_string(), item.file_size)
            };

            // Determine transfer method based on file size
            let use_high_speed = file_size > 500 * 1024 * 1024; // 500MB threshold for high-speed

            if use_high_speed {
                info!(
                    "🚀 Using HIGH-SPEED transfer for large file ({:.1}MB)",
                    file_size as f64 / (1024.0 * 1024.0)
                );

                // Check if high-speed manager is available
                let hs_manager = high_speed_manager.read().await;
                if let Some(ref manager) = *hs_manager {
                    // Get peer address
                    let peer_addr = {
                        let pm = peer_manager.read().await;
                        pm.get_peer_address(source_device)
                    };

                    if let Some(addr) = peer_addr {
                        // Send high-speed file request
                        let request_id = uuid::Uuid::new_v4();
                        let message = crate::network::protocol::Message::new(
                            crate::network::protocol::MessageType::HighSpeedFileRequest {
                                request_id,
                                file_path: source_file_path,
                                target_path: target_path.to_string_lossy().to_string(),
                                suggested_connections: 6,
                            },
                        );

                        let mut pm = peer_manager.write().await;
                        match pm.send_message_to_peer(source_device, message).await {
                            Ok(()) => {
                                let file_size_mb = file_size as f64 / (1024.0 * 1024.0);

                                notify_rust::Notification::new()
                                    .summary("🚀 HIGH-SPEED Transfer Starting")
                                    .body(&format!(
                                        "Requesting large file ({:.1} MB)\n⚡ HIGH-SPEED mode\nFrom: {}\nTo: {}",
                                        file_size_mb,
                                        source_device.to_string().chars().take(8).collect::<String>(),
                                        target_path.file_name().unwrap_or_default().to_string_lossy()
                                    ))
                                    .timeout(notify_rust::Timeout::Milliseconds(5000))
                                    .show()
                                    .map_err(|e| crate::FileshareError::Unknown(format!("Notification error: {}", e)))?;

                                info!("✅ HIGH-SPEED file request sent to source device");
                            }
                            Err(e) => {
                                error!("❌ Failed to send HIGH-SPEED file request: {}", e);

                                notify_rust::Notification::new()
                                    .summary("❌ HIGH-SPEED Transfer Failed")
                                    .body(&format!("Could not contact source device: {}", e))
                                    .timeout(notify_rust::Timeout::Milliseconds(5000))
                                    .show()
                                    .map_err(|e| {
                                        crate::FileshareError::Unknown(format!(
                                            "Notification error: {}",
                                            e
                                        ))
                                    })?;

                                return Err(e);
                            }
                        }
                    } else {
                        return Err(crate::FileshareError::Transfer(
                            "Peer address not found".to_string(),
                        ));
                    }
                } else {
                    warn!("HIGH-SPEED manager not available, falling back to regular transfer");
                    // Fall back to regular transfer logic
                    return Self::handle_regular_file_request(
                        peer_manager,
                        source_device,
                        source_file_path,
                        target_path,
                        file_size,
                    )
                    .await;
                }
            } else {
                // Use regular transfer for smaller files
                return Self::handle_regular_file_request(
                    peer_manager,
                    source_device,
                    source_file_path,
                    target_path,
                    file_size,
                )
                .await;
            }
        }

        Ok(())
    }

    // Regular file request handler (fallback)
    async fn handle_regular_file_request(
        peer_manager: Arc<RwLock<PeerManager>>,
        source_device: uuid::Uuid,
        source_file_path: String,
        target_path: std::path::PathBuf,
        file_size: u64,
    ) -> Result<()> {
        info!(
            "📁 Using regular transfer for file ({:.1}MB)",
            file_size as f64 / (1024.0 * 1024.0)
        );

        let request_id = uuid::Uuid::new_v4();
        let message = crate::network::protocol::Message::new(
            crate::network::protocol::MessageType::FileRequest {
                request_id,
                file_path: source_file_path,
                target_path: target_path.to_string_lossy().to_string(),
            },
        );

        let mut pm = peer_manager.write().await;
        pm.send_message_to_peer(source_device, message).await?;

        notify_rust::Notification::new()
            .summary("📁 File Transfer Starting")
            .body("Requesting file from source device...")
            .timeout(notify_rust::Timeout::Milliseconds(3000))
            .show()
            .map_err(|e| crate::FileshareError::Unknown(format!("Notification error: {}", e)))?;

        Ok(())
    }

    // Enhanced peer manager with high-speed streaming support
    async fn run_enhanced_peer_manager_with_high_speed(
        peer_manager: Arc<RwLock<PeerManager>>,
        settings: Arc<Settings>,
        clipboard: ClipboardManager,
        high_speed_manager: Arc<RwLock<Option<HighSpeedStreamingManager>>>,
    ) -> Result<()> {
        let listener = tokio::net::TcpListener::bind(settings.get_bind_address()).await?;
        info!(
            "🌐 Enhanced peer manager with HIGH-SPEED support listening on {}",
            settings.get_bind_address()
        );

        // Health monitoring (less frequent for large transfers)
        let health_pm = peer_manager.clone();
        let health_monitor = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(90)); // Increased for high-speed transfers
            loop {
                interval.tick().await;

                let mut pm = health_pm.write().await;
                if let Err(e) = pm.check_peer_health_all().await {
                    error!("❌ Health monitoring error: {}", e);
                }

                let stats = pm.get_connection_stats();
                debug!(
                    "📊 Connection Stats: {} total, {} healthy, {} unhealthy",
                    stats.total, stats.authenticated, stats.unhealthy
                );
            }
        });

        // Transfer monitoring (optimized for high-speed)
        let transfer_pm = peer_manager.clone();
        let transfer_hs_manager = high_speed_manager.clone();
        let transfer_monitor = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(45)); // Less frequent for high-speed
            loop {
                interval.tick().await;

                // Monitor regular transfers
                let pm = transfer_pm.read().await;
                let mut ft = pm.file_transfer.write().await;

                if let Err(e) = ft.monitor_transfer_health().await {
                    error!("❌ Transfer health monitoring error: {}", e);
                }

                let transfer_count = ft.active_transfers.len();
                if transfer_count > 0 {
                    debug!("📊 Regular transfers: {}", transfer_count);
                    if transfer_count > 10 {
                        ft.cleanup_stale_transfers_enhanced();
                    }
                }
                drop(ft);
                drop(pm);

                // Monitor high-speed transfers
                let hs_manager_guard = transfer_hs_manager.read().await;
                if let Some(ref hs_manager) = *hs_manager_guard {
                    // Add high-speed transfer monitoring here
                    debug!("📊 HIGH-SPEED transfers active");
                }
            }
        });

        // Enhanced message processing with high-speed support
        let message_pm = peer_manager.clone();
        let message_clipboard = clipboard.clone();
        let message_hs_manager = high_speed_manager.clone();
        let message_handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(5)); // Very fast for high-speed
            loop {
                interval.tick().await;

                let mut pm = message_pm.write().await;
                let mut processed_count = 0;

                while let Ok((peer_id, message)) = pm.message_rx.try_recv() {
                    processed_count += 1;

                    // Enhanced message routing with high-speed support
                    if !pm.is_peer_healthy(peer_id) {
                        warn!("⚠️ Dropping message from unhealthy peer {}", peer_id);
                        continue;
                    }

                    // Handle high-speed specific messages
                    match &message.message_type {
                        crate::network::protocol::MessageType::HighSpeedFileRequest { .. } => {
                            info!("🚀 Processing HIGH-SPEED file request from {}", peer_id);
                            // Handle high-speed file request
                            if let Err(e) = Self::handle_high_speed_file_request(
                                &mut pm,
                                peer_id,
                                message,
                                &message_hs_manager,
                            )
                            .await
                            {
                                error!("❌ Failed to handle HIGH-SPEED request: {}", e);
                            }
                            continue;
                        }

                        crate::network::protocol::MessageType::HighSpeedFileOffer { .. } => {
                            info!("🚀 Processing HIGH-SPEED file offer from {}", peer_id);
                            // Handle high-speed file offer
                            continue;
                        }

                        // Route regular transfer messages
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
                        } => {
                            // Enhanced routing logic for regular transfers
                            let ft = pm.file_transfer.read().await;
                            let is_outgoing = ft.has_transfer(*transfer_id)
                                && matches!(
                                    ft.get_transfer_direction(*transfer_id),
                                    Some(TransferDirection::Outgoing)
                                );
                            drop(ft);

                            if is_outgoing {
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

                    // Prevent monopolizing the loop
                    if processed_count >= 200 {
                        // Higher for high-speed
                        break;
                    }
                }

                if processed_count > 0 {
                    debug!("📨 Processed {} messages", processed_count);
                }
            }
        });

        // Connection handling
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

        // Wait for any service to complete
        tokio::select! {
            _ = connection_handle => info!("Connection handler stopped"),
            _ = message_handle => info!("Message handler stopped"),
            _ = health_monitor => info!("Health monitor stopped"),
            _ = transfer_monitor => info!("Transfer monitor stopped"),
        }

        Ok(())
    }

    // Handle high-speed file requests
    async fn handle_high_speed_file_request(
        pm: &mut crate::network::PeerManager,
        peer_id: uuid::Uuid,
        message: crate::network::protocol::Message,
        high_speed_manager: &Arc<RwLock<Option<HighSpeedStreamingManager>>>,
    ) -> Result<()> {
        if let crate::network::protocol::MessageType::HighSpeedFileRequest {
            request_id,
            file_path,
            target_path,
            suggested_connections,
        } = message.message_type
        {
            info!(
                "🚀 HIGH-SPEED file request: {} -> {} (connections: {})",
                file_path, target_path, suggested_connections
            );

            // Validate file exists
            let source_path = std::path::PathBuf::from(&file_path);
            if !source_path.exists() {
                let error_response = crate::network::protocol::Message::new(
                    crate::network::protocol::MessageType::HighSpeedTransferError {
                        transfer_id: request_id,
                        error: "File not found".to_string(),
                        connection_id: None,
                    },
                );
                pm.send_message_to_peer(peer_id, error_response).await?;
                return Ok(());
            }

            // Get file size
            let file_size = tokio::fs::metadata(&source_path).await?.len();

            // Check if high-speed manager is available
            let hs_manager = high_speed_manager.read().await;
            if let Some(ref manager) = *hs_manager {
                // Get peer address
                if let Some(peer_addr) = pm.get_peer_address(peer_id) {
                    info!(
                        "🚀 Starting HIGH-SPEED transfer: {:.1}MB to {}",
                        file_size as f64 / (1024.0 * 1024.0),
                        peer_addr
                    );

                    // Start high-speed transfer
                    match manager
                        .start_outgoing_transfer(request_id, peer_id, source_path, peer_addr)
                        .await
                    {
                        Ok(()) => {
                            info!("✅ HIGH-SPEED transfer started successfully");

                            // Send success response
                            let response = crate::network::protocol::Message::new(
                                crate::network::protocol::MessageType::HighSpeedFileOfferResponse {
                                    transfer_id: request_id,
                                    accepted: true,
                                    reason: None,
                                    my_connections: vec![peer_addr], // Simplified
                                },
                            );
                            pm.send_message_to_peer(peer_id, response).await?;
                        }
                        Err(e) => {
                            error!("❌ Failed to start HIGH-SPEED transfer: {}", e);

                            let error_response = crate::network::protocol::Message::new(
                                crate::network::protocol::MessageType::HighSpeedTransferError {
                                    transfer_id: request_id,
                                    error: e.to_string(),
                                    connection_id: None,
                                },
                            );
                            pm.send_message_to_peer(peer_id, error_response).await?;
                        }
                    }
                } else {
                    let error_response = crate::network::protocol::Message::new(
                        crate::network::protocol::MessageType::HighSpeedTransferError {
                            transfer_id: request_id,
                            error: "Peer address not found".to_string(),
                            connection_id: None,
                        },
                    );
                    pm.send_message_to_peer(peer_id, error_response).await?;
                }
            } else {
                // High-speed manager not available, fall back to regular transfer
                warn!("HIGH-SPEED manager not available, falling back to regular transfer");

                let regular_request = crate::network::protocol::Message::new(
                    crate::network::protocol::MessageType::FileRequest {
                        request_id,
                        file_path,
                        target_path,
                    },
                );

                // Process as regular file request
                pm.handle_message(
                    peer_id,
                    regular_request,
                    &crate::clipboard::ClipboardManager::new(pm.settings.device.id),
                )
                .await?;
            }
        }

        Ok(())
    }

    // Get method to get discovered devices (for UI access)
    pub async fn get_discovered_devices(&self) -> Vec<crate::network::discovery::DeviceInfo> {
        let pm = self.peer_manager.read().await;
        pm.get_all_discovered_devices().await
    }

    // Get high-speed transfer progress
    pub async fn get_high_speed_progress(
        &self,
        transfer_id: uuid::Uuid,
    ) -> Option<HighSpeedProgress> {
        let hs_manager = self.high_speed_manager.read().await;
        if let Some(ref manager) = *hs_manager {
            manager.get_progress(transfer_id).await
        } else {
            None
        }
    }

    // Get all high-speed transfer progress
    pub async fn get_all_high_speed_progress(&self) -> Vec<HighSpeedProgress> {
        let hs_manager = self.high_speed_manager.read().await;
        if let Some(ref manager) = *hs_manager {
            // This would need to be implemented in the HighSpeedStreamingManager
            vec![] // Placeholder
        } else {
            vec![]
        }
    }
}
