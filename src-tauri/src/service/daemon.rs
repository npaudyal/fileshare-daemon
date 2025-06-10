use crate::{
    clipboard::ClipboardManager,
    config::Settings,
    hotkeys::{HotkeyEvent, HotkeyManager},
    network::{DiscoveryService, MessageType, PeerManager},
    service::file_transfer::TransferDirection,
    utils::format_file_size,
    Result,
};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, RwLock};
use tracing::{error, info, warn};

pub struct FileshareDaemon {
    settings: Arc<Settings>,
    pub discovery: Option<DiscoveryService>,
    pub peer_manager: Arc<RwLock<PeerManager>>,
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

        // Initialize hotkey manager (FIXED: using simple version)
        let hotkey_manager = HotkeyManager::new()?;

        // Initialize clipboard manager with device ID
        let clipboard = ClipboardManager::new(settings.device.id);

        Ok(Self {
            settings,
            discovery: Some(discovery),
            peer_manager,
            hotkey_manager,
            clipboard,
            shutdown_tx,
            shutdown_rx,
        })
    }

    // Method to get the hotkey event sender
    pub fn get_hotkey_event_sender(&self) -> mpsc::UnboundedSender<HotkeyEvent> {
        self.hotkey_manager.get_event_sender()
    }

    // Method to get discovered devices
    pub async fn get_discovered_devices(&self) -> Vec<crate::network::discovery::DeviceInfo> {
        let pm = self.peer_manager.read().await;
        pm.get_all_discovered_devices().await
    }

    pub async fn run(mut self) -> Result<()> {
        info!("🚀 Starting Fileshare Daemon...");
        info!("📱 Device ID: {}", self.settings.device.id);
        info!("🏷️ Device Name: {}", self.settings.device.name);
        info!("🌐 Listening on port: {}", self.settings.network.port);

        // CRITICAL FIX: Start hotkey manager FIRST with proper threading context
        info!("🎹 Initializing hotkey manager...");
        self.hotkey_manager.start().await?;
        info!("✅ Hotkey manager started successfully");

        // Create a JoinSet to manage all background tasks
        let mut task_set = tokio::task::JoinSet::new();

        // Start discovery service
        if let Some(mut discovery) = self.discovery.take() {
            task_set.spawn(async move {
                info!("🔍 Starting discovery service...");

                // FIXED: Discovery service should run forever, if it stops it's an error
                loop {
                    match discovery.run().await {
                        Ok(_) => {
                            error!("❌ Discovery service stopped unexpectedly, restarting...");
                            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                        }
                        Err(e) => {
                            error!("❌ Discovery service error: {}, restarting in 5s...", e);
                            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                        }
                    }
                }
            });
        } else {
            error!("❌ Discovery service not available");
            return Err(crate::FileshareError::Unknown(
                "Discovery service not available".to_string(),
            ));
        }

        // Start peer manager
        {
            let peer_manager = self.peer_manager.clone();
            let settings = self.settings.clone();
            let clipboard = self.clipboard.clone();
            let mut shutdown_rx = self.shutdown_tx.subscribe();

            task_set.spawn(async move {
                loop {
                    tokio::select! {
                        result = Self::run_peer_manager(peer_manager.clone(), settings.clone(), clipboard.clone()) => {
                            match result {
                                Ok(_) => {
                                    error!("❌ Peer manager stopped unexpectedly, restarting...");
                                }
                                Err(e) => {
                                    error!("❌ Peer manager error: {}, restarting in 5s...", e);
                                }
                            }
                            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                        }
                        _ = shutdown_rx.recv() => {
                            info!("🛑 Peer manager shutdown requested");
                            break;
                        }
                    }
                }
            });
        }

        // Start hotkey event handler
        {
            let peer_manager = self.peer_manager.clone();
            let clipboard = self.clipboard.clone();
            let mut hotkey_manager = self.hotkey_manager;
            let mut shutdown_rx = self.shutdown_tx.subscribe();

            task_set.spawn(async move {
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
            });
        }

        info!("✅ All background services started successfully");

        // FIXED: Only shut down on explicit signals, not on service completion
        let mut shutdown_rx = self.shutdown_rx;

        tokio::select! {
            _ = shutdown_rx.recv() => {
                info!("🛑 Shutdown signal received");
            }
            _ = tokio::signal::ctrl_c() => {
                info!("🛑 Ctrl+C received, shutting down");
            }
            // DON'T shut down if a service "completes" - services should run forever
            // Only shut down on explicit shutdown signals
        }

        // Clean shutdown - abort all remaining tasks
        info!("🛑 Shutting down services...");
        task_set.abort_all();

        // Wait for all tasks to finish aborting
        while let Some(result) = task_set.join_next().await {
            match result {
                Ok(_) => info!("✅ Task shut down cleanly"),
                Err(e) if e.is_cancelled() => info!("✅ Task cancelled during shutdown"),
                Err(e) => warn!("⚠️ Task error during shutdown: {}", e),
            }
        }

        info!("✅ Fileshare Daemon stopped");
        Ok(())
    }

    // FIXED: Simplified hotkey event handler (no complex Windows logic)
    async fn handle_hotkey_events(
        hotkey_manager: &mut HotkeyManager,
        peer_manager: Arc<RwLock<PeerManager>>,
        clipboard: ClipboardManager,
    ) {
        info!("🎹 Starting hotkey event handler");

        loop {
            if let Some(event) = hotkey_manager.get_event().await {
                match event {
                    HotkeyEvent::CopyFiles => {
                        info!(
                            "📋 Copy hotkey triggered - copying selected file to network clipboard"
                        );
                        if let Err(e) =
                            Self::handle_copy_operation(clipboard.clone(), peer_manager.clone())
                                .await
                        {
                            error!("❌ Failed to handle copy operation: {}", e);
                        }
                    }
                    HotkeyEvent::PasteFiles => {
                        info!("📁 Paste hotkey triggered - pasting from network clipboard");
                        if let Err(e) =
                            Self::handle_paste_operation(clipboard.clone(), peer_manager.clone())
                                .await
                        {
                            error!("❌ Failed to handle paste operation: {}", e);
                        }
                    }
                }
            }
        }
    }

    async fn handle_copy_operation(
        clipboard: ClipboardManager,
        peer_manager: Arc<RwLock<PeerManager>>,
    ) -> Result<()> {
        info!("📋 Handling copy operation");

        // Copy currently selected file to network clipboard
        clipboard.copy_selected_file().await?;

        // Get the clipboard item to broadcast to other devices
        let clipboard_item = {
            let clipboard_state = clipboard.network_clipboard.read().await;
            clipboard_state.clone()
        };

        if let Some(item) = clipboard_item {
            // Broadcast clipboard update to all connected peers
            let peers = {
                let pm = peer_manager.read().await;
                pm.get_connected_peers()
            };

            for peer in peers {
                let message = crate::network::protocol::Message::new(
                    crate::network::protocol::MessageType::ClipboardUpdate {
                        file_path: item.file_path.to_string_lossy().to_string(),
                        source_device: item.source_device,
                        timestamp: item.timestamp,
                        file_size: item.file_size,
                    },
                );

                let mut pm = peer_manager.write().await;
                if let Err(e) = pm.send_message_to_peer(peer.device_info.id, message).await {
                    warn!(
                        "❌ Failed to send clipboard update to {}: {}",
                        peer.device_info.id, e
                    );
                }
            }

            // Show success notification
            let filename = item
                .file_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy();

            notify_rust::Notification::new()
                .summary("File Copied to Network")
                .body(&format!(
                    "Copied: {} ({})",
                    filename,
                    format_file_size(item.file_size)
                ))
                .timeout(notify_rust::Timeout::Milliseconds(3000))
                .show()
                .map_err(|e| {
                    crate::FileshareError::Unknown(format!("Notification error: {}", e))
                })?;

            info!("✅ Copy operation completed successfully");
        }

        Ok(())
    }

    async fn handle_paste_operation(
        clipboard: ClipboardManager,
        peer_manager: Arc<RwLock<PeerManager>>,
    ) -> Result<()> {
        info!("📁 Handling paste operation");

        // Try to paste from network clipboard
        if let Some((target_path, source_device)) = clipboard.paste_to_current_location().await? {
            info!(
                "📁 Requesting file transfer from device {} to {:?}",
                source_device, target_path
            );

            // Get the source file path from clipboard
            let source_file_path = {
                let clipboard_state = clipboard.network_clipboard.read().await;
                clipboard_state
                    .as_ref()
                    .unwrap()
                    .file_path
                    .to_string_lossy()
                    .to_string()
            };

            // Send file request to source device
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

            // Show notification that transfer is starting
            notify_rust::Notification::new()
                .summary("File Transfer Starting")
                .body("Requesting file from source device...")
                .timeout(notify_rust::Timeout::Milliseconds(3000))
                .show()
                .map_err(|e| {
                    crate::FileshareError::Unknown(format!("Notification error: {}", e))
                })?;

            info!("✅ File request sent to source device");
        }

        Ok(())
    }

    async fn run_peer_manager(
        peer_manager: Arc<RwLock<PeerManager>>,
        settings: Arc<Settings>,
        clipboard: ClipboardManager,
    ) -> Result<()> {
        let listener = tokio::net::TcpListener::bind(settings.get_bind_address()).await?;
        info!(
            "🌐 Peer manager listening on {}",
            settings.get_bind_address()
        );

        // Spawn a task to handle incoming connections
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
                        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                    }
                }
            }
        });

        // Message handling task with proper transfer routing
        let message_pm = peer_manager.clone();
        let message_clipboard = clipboard.clone();
        let message_handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(100));
            loop {
                interval.tick().await;

                let mut pm = message_pm.write().await;

                while let Ok((peer_id, message)) = pm.message_rx.try_recv() {
                    // Route outgoing transfer messages directly to avoid loops
                    match &message.message_type {
                        MessageType::FileOffer { transfer_id, .. } => {
                            let is_our_outgoing = {
                                let ft = pm.file_transfer.read().await;
                                ft.has_transfer(*transfer_id)
                                    && matches!(
                                        ft.get_transfer_direction(*transfer_id),
                                        Some(TransferDirection::Outgoing)
                                    )
                            };

                            if is_our_outgoing {
                                if let Err(e) = pm.send_direct_to_connection(peer_id, message).await
                                {
                                    error!(
                                        "❌ Failed to send FileOffer to peer {}: {}",
                                        peer_id, e
                                    );
                                }
                                continue;
                            }
                        }

                        MessageType::FileChunk { transfer_id, .. } => {
                            let is_our_outgoing = {
                                let ft = pm.file_transfer.read().await;
                                ft.has_transfer(*transfer_id)
                                    && matches!(
                                        ft.get_transfer_direction(*transfer_id),
                                        Some(TransferDirection::Outgoing)
                                    )
                            };

                            if is_our_outgoing {
                                if let Err(e) = pm.send_direct_to_connection(peer_id, message).await
                                {
                                    error!(
                                        "❌ Failed to send FileChunk to peer {}: {}",
                                        peer_id, e
                                    );
                                }
                                continue;
                            }
                        }

                        MessageType::TransferComplete { transfer_id, .. } => {
                            let is_our_outgoing = {
                                let ft = pm.file_transfer.read().await;
                                ft.has_transfer(*transfer_id)
                                    && matches!(
                                        ft.get_transfer_direction(*transfer_id),
                                        Some(TransferDirection::Outgoing)
                                    )
                            };

                            if is_our_outgoing {
                                if let Err(e) = pm.send_direct_to_connection(peer_id, message).await
                                {
                                    error!(
                                        "❌ Failed to send TransferComplete to peer {}: {}",
                                        peer_id, e
                                    );
                                }
                                continue;
                            }
                        }

                        MessageType::TransferError { transfer_id, .. } => {
                            let is_our_outgoing = {
                                let ft = pm.file_transfer.read().await;
                                ft.has_transfer(*transfer_id)
                                    && matches!(
                                        ft.get_transfer_direction(*transfer_id),
                                        Some(TransferDirection::Outgoing)
                                    )
                            };

                            if is_our_outgoing {
                                if let Err(e) = pm.send_direct_to_connection(peer_id, message).await
                                {
                                    error!(
                                        "❌ Failed to send TransferError to peer {}: {}",
                                        peer_id, e
                                    );
                                }
                                continue;
                            }
                        }

                        _ => {
                            // All non-transfer messages process normally
                        }
                    }

                    // Process all other messages normally
                    if let Err(e) = pm
                        .handle_message(peer_id, message, &message_clipboard)
                        .await
                    {
                        error!("❌ Error processing message: {}", e);
                    }
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
