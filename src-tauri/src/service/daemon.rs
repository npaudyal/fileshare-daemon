// src/service/daemon.rs - SIMPLIFIED VERSION
use crate::{
    clipboard::ClipboardManager,
    config::Settings,
    hotkeys::{HotkeyEvent, HotkeyManager},
    network::{DiscoveryService, PeerManager},
    service::adaptive_transfer::AdaptiveTransferManager,
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

    // SIMPLIFIED: Single adaptive transfer manager
    pub transfer_manager: Arc<RwLock<AdaptiveTransferManager>>,

    hotkey_manager: Option<HotkeyManager>,
    clipboard: ClipboardManager,
    shutdown_tx: broadcast::Sender<()>,
    shutdown_rx: broadcast::Receiver<()>,
}

impl FileshareDaemon {
    pub async fn new(settings: Settings) -> Result<Self> {
        let settings = Arc::new(settings);
        let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

        // Create peer manager
        let mut peer_manager = PeerManager::new(settings.clone()).await?;
        let peer_manager = Arc::new(RwLock::new(peer_manager));

        // Create simplified transfer manager
        let transfer_manager = AdaptiveTransferManager::new(settings.clone()).await?;
        let transfer_manager = Arc::new(RwLock::new(transfer_manager));

        // FIXED: Set up transfer manager integration with peer manager (async)
        {
            let mut pm = peer_manager.write().await;
            pm.set_transfer_manager(transfer_manager.clone()).await; // FIXED: Add .await
        }

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

        info!("✅ Simplified FileshareDaemon initialized with integrated transfer manager");

        Ok(Self {
            settings,
            discovery: Some(discovery),
            peer_manager,
            transfer_manager,
            hotkey_manager: Some(hotkey_manager),
            clipboard,
            shutdown_tx,
            shutdown_rx,
        })
    }

    pub async fn start_background_services(mut self: Arc<Self>) -> Result<()> {
        info!("🚀 Starting Simplified Fileshare Daemon...");
        info!("📱 Device ID: {}", self.settings.device.id);
        info!("🏷️ Device Name: {}", self.settings.device.name);
        info!("🌐 Listening on port: {}", self.settings.network.port);

        {
            let mut tm = self.transfer_manager.write().await;
            tm.start_transfer_server().await?;
            info!("✅ Transfer server started");
        }

        // Start hotkey manager
        let mut hotkey_manager = HotkeyManager::new()?;
        let peer_manager_for_hotkeys = self.peer_manager.clone();
        let transfer_manager_for_hotkeys = self.transfer_manager.clone();
        let clipboard_for_hotkeys = self.clipboard.clone();

        tokio::spawn(async move {
            if let Err(e) = hotkey_manager.start().await {
                error!("❌ Failed to start hotkey manager: {}", e);
                return;
            }
            info!("✅ Hotkey manager started successfully");

            Self::handle_hotkey_events_simplified(
                &mut hotkey_manager,
                peer_manager_for_hotkeys,
                transfer_manager_for_hotkeys,
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

        // Start simplified peer manager
        let peer_manager = self.peer_manager.clone();
        let transfer_manager = self.transfer_manager.clone();
        let settings = self.settings.clone();
        let clipboard = self.clipboard.clone();

        tokio::spawn(async move {
            if let Err(e) = Self::run_simplified_peer_manager(
                peer_manager,
                transfer_manager,
                settings,
                clipboard,
            )
            .await
            {
                error!("❌ Simplified peer manager error: {}", e);
            }
        });

        info!("✅ All simplified background services started successfully");
        Ok(())
    }

    pub async fn get_discovered_devices(&self) -> Vec<crate::network::discovery::DeviceInfo> {
        let pm = self.peer_manager.read().await;
        pm.get_all_discovered_devices().await
    }

    /// Get transfer progress from adaptive transfer manager
    pub async fn get_transfer_progress(
        &self,
        transfer_id: uuid::Uuid,
    ) -> Option<crate::service::adaptive_transfer::TransferProgress> {
        let tm = self.transfer_manager.read().await;
        tm.get_progress(transfer_id)
    }

    /// Get all active transfer progress
    pub async fn get_all_active_transfers(
        &self,
    ) -> Vec<crate::service::adaptive_transfer::TransferProgress> {
        let tm = self.transfer_manager.read().await;
        tm.get_all_active_progress()
    }

    /// Cancel a transfer
    pub async fn cancel_transfer(&self, transfer_id: uuid::Uuid) -> Result<()> {
        let mut tm = self.transfer_manager.write().await;
        tm.cancel_transfer(transfer_id)
    }

    /// Get connection statistics
    pub async fn get_connection_stats(&self) -> crate::network::peer::ConnectionStats {
        let pm = self.peer_manager.read().await;
        pm.get_connection_stats()
    }

    /// Send file to peer
    pub async fn send_file_to_peer(
        &self,
        peer_id: uuid::Uuid,
        file_path: std::path::PathBuf,
    ) -> Result<()> {
        let mut pm = self.peer_manager.write().await;
        pm.send_file_to_peer(peer_id, file_path).await
    }

    /// Get peer health status
    pub async fn is_peer_healthy(&self, peer_id: uuid::Uuid) -> bool {
        let pm = self.peer_manager.read().await;
        pm.is_peer_healthy(peer_id)
    }

    /// Force peer health check
    pub async fn check_peer_health(&self) -> Result<()> {
        let mut pm = self.peer_manager.write().await;
        pm.check_peer_health_all().await
    }

    /// Cleanup completed transfers
    pub async fn cleanup_transfers(&self) {
        let mut tm = self.transfer_manager.write().await;
        tm.cleanup_completed_transfers();

        let mut pm = self.peer_manager.write().await;
        pm.cleanup_pending_transfers();
    }

    // Simplified hotkey handler
    async fn handle_hotkey_events_simplified(
        hotkey_manager: &mut HotkeyManager,
        peer_manager: Arc<RwLock<PeerManager>>,
        transfer_manager: Arc<RwLock<AdaptiveTransferManager>>,
        clipboard: ClipboardManager,
    ) {
        info!("🎹 Simplified hotkey event handler active...");

        loop {
            if let Some(event) = hotkey_manager.get_event().await {
                match event {
                    HotkeyEvent::CopyFiles => {
                        info!("📋 Copy hotkey triggered");
                        if let Err(e) = Self::handle_copy_operation_simplified(
                            clipboard.clone(),
                            peer_manager.clone(),
                        )
                        .await
                        {
                            error!("❌ Failed to handle copy operation: {}", e);
                        }
                    }
                    HotkeyEvent::PasteFiles => {
                        info!("📁 Paste hotkey triggered");
                        if let Err(e) = Self::handle_paste_operation_simplified(
                            clipboard.clone(),
                            peer_manager.clone(),
                            transfer_manager.clone(),
                        )
                        .await
                        {
                            error!("❌ Failed to handle paste operation: {}", e);
                        }
                    }
                }
            } else {
                warn!("🎹 Hotkey event channel closed, stopping handler");
                break;
            }
        }
    }

    // Simplified copy operation
    async fn handle_copy_operation_simplified(
        clipboard: ClipboardManager,
        peer_manager: Arc<RwLock<PeerManager>>,
    ) -> Result<()> {
        info!("📋 Handling simplified copy operation");

        // Copy file to network clipboard
        clipboard.copy_selected_file().await?;

        // Get clipboard item
        let clipboard_item = {
            let clipboard_state = clipboard.network_clipboard.read().await;
            clipboard_state.clone()
        };

        if let Some(item) = clipboard_item {
            // Broadcast clipboard update
            let healthy_peer_ids = {
                let pm = peer_manager.read().await;
                pm.get_connected_peers()
                    .iter()
                    .filter(|peer| pm.is_peer_healthy(peer.device_info.id))
                    .map(|peer| peer.device_info.id)
                    .collect::<Vec<_>>()
            };

            info!(
                "📡 Broadcasting to {} healthy peers",
                healthy_peer_ids.len()
            );

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

            // Show notification
            let filename = item
                .file_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy();
            let file_size_mb = item.file_size as f64 / (1024.0 * 1024.0);

            notify_rust::Notification::new()
                .summary("File Copied to Network")
                .body(&format!("📁 {} ({:.1} MB)", filename, file_size_mb))
                .timeout(notify_rust::Timeout::Milliseconds(3000))
                .show()
                .map_err(|e| {
                    crate::FileshareError::Unknown(format!("Notification error: {}", e))
                })?;

            info!("✅ Simplified copy operation completed");
        }

        Ok(())
    }

    // Simplified paste operation
    async fn handle_paste_operation_simplified(
        clipboard: ClipboardManager,
        peer_manager: Arc<RwLock<PeerManager>>,
        transfer_manager: Arc<RwLock<AdaptiveTransferManager>>,
    ) -> Result<()> {
        info!("📁 Handling simplified paste operation");

        if let Some((target_path, source_device)) = clipboard.paste_to_current_location().await? {
            // Get source file info
            let (source_file_path, file_size) = {
                let clipboard_state = clipboard.network_clipboard.read().await;
                let item = clipboard_state.as_ref().unwrap();
                (item.file_path.to_string_lossy().to_string(), item.file_size)
            };

            info!(
                "🎯 Requesting file: {} ({:.1}MB)",
                source_file_path,
                file_size as f64 / (1024.0 * 1024.0)
            );

            // Send simplified file request
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
                .map_err(|e| {
                    crate::FileshareError::Unknown(format!("Notification error: {}", e))
                })?;

            info!("✅ File request sent to source device");
        }

        Ok(())
    }

    // Simplified peer manager
    async fn run_simplified_peer_manager(
        peer_manager: Arc<RwLock<PeerManager>>,
        transfer_manager: Arc<RwLock<AdaptiveTransferManager>>,
        settings: Arc<Settings>,
        clipboard: ClipboardManager,
    ) -> Result<()> {
        let listener = tokio::net::TcpListener::bind(settings.get_bind_address()).await?;
        info!(
            "🌐 Simplified peer manager listening on {}",
            settings.get_bind_address()
        );

        // Health monitoring
        let health_pm = peer_manager.clone();
        let health_monitor = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            loop {
                interval.tick().await;
                let mut pm = health_pm.write().await;
                if let Err(e) = pm.check_peer_health_all().await {
                    error!("❌ Health monitoring error: {}", e);
                }
            }
        });

        // Transfer cleanup
        let cleanup_tm = transfer_manager.clone();
        let cleanup_monitor = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            loop {
                interval.tick().await;
                let mut tm = cleanup_tm.write().await;
                tm.cleanup_completed_transfers();
            }
        });

        // Message processing
        let message_pm = peer_manager.clone();
        let message_tm = transfer_manager.clone();
        let message_clipboard = clipboard.clone();
        let message_handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(10));
            loop {
                interval.tick().await;

                let mut pm = message_pm.write().await;
                let mut processed_count = 0;

                while let Ok((peer_id, message)) = pm.message_rx.try_recv() {
                    processed_count += 1;

                    if !pm.is_peer_healthy(peer_id) {
                        warn!("⚠️ Dropping message from unhealthy peer {}", peer_id);
                        continue;
                    }

                    // Handle simplified file transfer messages
                    match &message.message_type {
                        crate::network::protocol::MessageType::SimpleFileOffer {
                            transfer_id,
                            metadata,
                        } => {
                            let mut tm = message_tm.write().await;
                            if let Err(e) = tm
                                .handle_file_offer(peer_id, *transfer_id, metadata.clone())
                                .await
                            {
                                error!("❌ Failed to handle file offer: {}", e);
                            }
                            continue;
                        }
                        crate::network::protocol::MessageType::SimpleFileOfferResponse {
                            transfer_id,
                            accepted,
                            my_port,
                        } => {
                            let mut tm = message_tm.write().await;
                            if let Err(e) = tm
                                .handle_offer_response(peer_id, *transfer_id, *accepted, *my_port)
                                .await
                            {
                                error!("❌ Failed to handle offer response: {}", e);
                            }
                            continue;
                        }
                        _ => {}
                    }

                    // Process other messages normally
                    if let Err(e) = pm
                        .handle_message(peer_id, message, &message_clipboard)
                        .await
                    {
                        error!("❌ Error processing message from peer {}: {}", peer_id, e);
                    }

                    if processed_count >= 100 {
                        break;
                    }
                }
            }
        });

        // UPDATED: Enhanced connection handling for adaptive transfers
        let connection_pm = peer_manager.clone();
        let connection_tm = transfer_manager.clone();
        let connection_handle = tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, addr)) => {
                        info!("🔗 New connection from {}", addr);

                        // Check if this is a regular peer connection or adaptive transfer connection
                        // For now, treat all as peer connections - adaptive transfers will use separate ports
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
            _ = cleanup_monitor => info!("Cleanup monitor stopped"),
        }

        Ok(())
    }
}
