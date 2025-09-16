use crate::{
    clipboard::ClipboardManager,
    config::Settings,
    hotkeys::{HotkeyEvent, HotkeyManager},
    network::{DiscoveryService, PeerManager},
    pairing::PairingManager,
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
    pub pairing_manager: Arc<RwLock<PairingManager>>,
    hotkey_manager: Option<HotkeyManager>,
    clipboard: ClipboardManager,
    shutdown_tx: broadcast::Sender<()>,
    shutdown_rx: broadcast::Receiver<()>,
}

impl FileshareDaemon {
    pub async fn new(settings: Settings) -> Result<Self> {
        let settings = Arc::new(settings);
        let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

        // Initialize peer manager with QUIC
        let mut peer_manager = PeerManager::new(settings.clone()).await?;

        // Get proper config path for keypair
        let keypair_path = if let Some(proj_dirs) = directories::ProjectDirs::from("com", "fileshare", "daemon") {
            let config_dir = proj_dirs.config_dir();
            std::fs::create_dir_all(config_dir).ok();
            config_dir.join("device_keypair.pem")
        } else {
            std::path::PathBuf::from("/tmp/device_keypair.pem")
        };

        // Initialize pairing manager
        let pairing_manager = PairingManager::new(
            settings.device.id,
            settings.device.name.clone(),
            keypair_path,
            settings.security.paired_devices.clone(),
        )?;
        let pairing_manager = Arc::new(RwLock::new(pairing_manager));

        // Set pairing manager in peer manager
        peer_manager.set_pairing_manager(pairing_manager.clone());
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
            pairing_manager,
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
    
    // Getter for settings (for UI access)
    pub fn get_settings(&self) -> &Arc<Settings> {
        &self.settings
    }

    // Enhanced daemon startup with health monitoring
    pub async fn start_background_services(mut self: Arc<Self>) -> Result<()> {
        info!("🚀 Starting Enhanced Fileshare Daemon with QUIC support...");
        info!("📱 Device ID: {}", self.settings.device.id);
        info!("🏷️ Device Name: {}", self.settings.device.name);
        info!("🌐 QUIC listening on port: {}", self.settings.network.port);

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

        // Start peer manager message handler
        let peer_manager = self.peer_manager.clone();
        let clipboard = self.clipboard.clone();
        let pairing_manager = self.pairing_manager.clone();
        let settings = self.settings.clone();
        tokio::spawn(async move {
            if let Err(e) = Self::run_quic_message_handler(peer_manager, clipboard, pairing_manager, settings).await {
                error!("❌ QUIC message handler error: {}", e);
            }
        });

        info!("✅ All enhanced background services started successfully");
        Ok(())
    }

    // QUIC message handler
    async fn run_quic_message_handler(
        peer_manager: Arc<RwLock<PeerManager>>,
        clipboard: ClipboardManager,
        pairing_manager: Arc<RwLock<PairingManager>>,
        settings: Arc<Settings>,
    ) -> Result<()> {
        info!("🚀 Starting QUIC message handler...");
        
        // Start health monitoring
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


        // Handle messages from peers
        // Extract the message receiver to avoid holding the lock while waiting
        let mut message_rx = {
            let mut pm = peer_manager.write().await;
            std::mem::replace(&mut pm.message_rx, tokio::sync::mpsc::unbounded_channel().1)
        };

        loop {
            // Wait for messages without holding any locks
            let (peer_id, message) = match message_rx.recv().await {
                Some((peer_id, message)) => (peer_id, message),
                None => {
                    warn!("Message channel closed");
                    break;
                }
            };

            info!("📨 Received message from {}: {:?}", peer_id, message.message_type);

            // Check if this is a pairing-related message that might result in completion
            let is_pairing_message = matches!(
                message.message_type,
                crate::network::protocol::MessageType::PairingComplete { .. } |
                crate::network::protocol::MessageType::PairingConfirm { .. }
            );

            // Only lock when handling the message
            let mut pm = peer_manager.write().await;
            if let Err(e) = pm.handle_message(peer_id, message, &clipboard).await {
                error!("❌ Failed to handle message from {}: {}", peer_id, e);
            }

            // Save paired devices to config after pairing completion
            if is_pairing_message {
                drop(pm); // Release the lock before saving config
                if let Err(e) = Self::save_paired_devices_to_config_static(&pairing_manager, &settings).await {
                    error!("❌ Failed to save paired devices to config: {}", e);
                }
            }
        }

        // Clean up
        health_monitor.abort();

        Ok(())
    }

    // Keep the existing run method for non-Tauri usage
    pub async fn run(mut self) -> Result<()> {
        info!("🚀 Starting Fileshare Daemon with QUIC...");
        info!("📱 Device ID: {}", self.settings.device.id);
        info!("🏷️ Device Name: {}", self.settings.device.name);
        info!("🌐 QUIC listening on port: {}", self.settings.network.port);

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

        // Start QUIC message handler
        let message_handle = {
            let peer_manager = self.peer_manager.clone();
            let clipboard = self.clipboard.clone();
            let mut shutdown_rx = self.shutdown_tx.subscribe();
            let pairing_manager = self.pairing_manager.clone();
            let settings = self.settings.clone();

            tokio::spawn(async move {
                tokio::select! {
                    result = Self::run_quic_message_handler(peer_manager, clipboard, pairing_manager, settings) => {
                        if let Err(e) = result {
                            error!("❌ QUIC message handler error: {}", e);
                        }
                    }
                    _ = shutdown_rx.recv() => {
                        info!("🛑 Message handler shutdown requested");
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
        message_handle.abort();
        hotkey_handle.abort();

        info!("✅ Fileshare Daemon stopped");
        Ok(())
    }

    async fn handle_hotkey_events(
        hotkey_manager: &mut HotkeyManager,
        peer_manager: Arc<RwLock<PeerManager>>,
        clipboard: ClipboardManager,
    ) {
        info!("🎹 Enhanced hotkey event handler active and listening...");

        loop {
            if let Some(event) = hotkey_manager.get_event().await {
                match event {
                    HotkeyEvent::CopyFiles => {
                        info!(
                            "📋 Copy hotkey triggered - copying selected file to network clipboard"
                        );
                        if let Err(e) = Self::handle_copy_operation(
                            clipboard.clone(),
                            peer_manager.clone(),
                        )
                        .await
                        {
                            error!("❌ Failed to handle copy operation: {}", e);
                        }
                    }
                    HotkeyEvent::PasteFiles => {
                        info!("📁 Paste hotkey triggered - pasting from network clipboard");
                        if let Err(e) = Self::handle_paste_operation(
                            clipboard.clone(),
                            peer_manager.clone(),
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

    async fn handle_copy_operation(
        clipboard: ClipboardManager,
        peer_manager: Arc<RwLock<PeerManager>>,
    ) -> Result<()> {
        info!("📋 Handling copy operation with QUIC");

        // Copy currently selected file to network clipboard
        match clipboard.copy_selected_file().await {
            Ok(()) => {
                info!("✅ File successfully copied to network clipboard");
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

        // Get the clipboard item to broadcast
        let clipboard_item = {
            let clipboard_state = clipboard.network_clipboard.read().await;
            clipboard_state.clone()
        };

        if let Some(item) = clipboard_item {
            let file_size_mb = item.file_size as f64 / (1024.0 * 1024.0);
            info!(
                "📊 Broadcasting file: {:.1} MB (QUIC streaming enabled)",
                file_size_mb
            );

            // Broadcast clipboard update to healthy peers
            let (healthy_peer_ids, peer_status_debug) = {
                let pm = peer_manager.read().await;
                let all_peers: Vec<_> = pm.peers.values().collect();
                let connected_peers = pm.get_connected_peers();
                let healthy_peer_ids: Vec<_> = connected_peers
                    .iter()
                    .filter(|peer| pm.is_peer_healthy(peer.device_info.id))
                    .map(|peer| peer.device_info.id)
                    .collect();
                
                let debug_info = format!(
                    "Total peers: {}, Connected: {}, Healthy: {}",
                    all_peers.len(),
                    connected_peers.len(),
                    healthy_peer_ids.len()
                );
                
                // Log individual peer statuses
                for peer in all_peers.iter() {
                    info!("🔍 Peer {}: {:?}", peer.device_info.name, peer.connection_status);
                }
                
                (healthy_peer_ids, debug_info)
            };

            let peer_count = healthy_peer_ids.len();
            info!("📡 Broadcasting to {} healthy peers via QUIC ({})", peer_count, peer_status_debug);

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

            // Show success notification
            let filename = item
                .file_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy();

            notify_rust::Notification::new()
                .summary("File Copied to Network")
                .body(&format!(
                    "✅ {}\n📦 Size: {:.1} MB\n📡 Shared with {} devices (QUIC)",
                    filename, file_size_mb, peer_count
                ))
                .timeout(notify_rust::Timeout::Milliseconds(4000))
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
        info!("📁 Handling paste operation with QUIC");

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
                "📁 Requesting QUIC file transfer from device {} to {:?}",
                source_device, target_path
            );

            // Get the source file path and validate size
            let (source_file_path, file_size) = {
                let clipboard_state = clipboard.network_clipboard.read().await;
                let item = clipboard_state.as_ref().unwrap();
                (item.file_path.to_string_lossy().to_string(), item.file_size)
            };

            let file_size_mb = file_size as f64 / (1024.0 * 1024.0);
            info!(
                "📊 Requesting file transfer: {:.1} MB (QUIC parallel streams)",
                file_size_mb
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

            let mut pm = peer_manager.write().await;
            pm.send_message_to_peer(source_device, message).await?;

            // Show notification that transfer is starting
            notify_rust::Notification::new()
                .summary("QUIC File Transfer Starting")
                .body("Requesting file from source device with parallel streams...")
                .timeout(notify_rust::Timeout::Milliseconds(3000))
                .show()
                .map_err(|e| {
                    crate::FileshareError::Unknown(format!("Notification error: {}", e))
                })?;

            info!("✅ File request sent to source device via QUIC");
        }

        Ok(())
    }

    /// Save paired devices to config file
    pub async fn save_paired_devices_to_config(&self) -> Result<()> {
        Self::save_paired_devices_to_config_static(&self.pairing_manager, &self.settings).await
    }

    /// Static version of save_paired_devices_to_config for use in message handler
    pub async fn save_paired_devices_to_config_static(
        pairing_manager: &Arc<RwLock<PairingManager>>,
        settings: &Arc<Settings>,
    ) -> Result<()> {
        // Get all paired devices from the pairing manager
        let paired_devices = {
            let pm = pairing_manager.read().await;
            pm.get_all_paired_devices().await
        };

        // Create a mutable copy of the settings
        let mut settings_copy = (**settings).clone();

        // Update the paired devices in settings
        settings_copy.security.paired_devices = paired_devices;

        // Save to config file
        settings_copy.save(None)
            .map_err(|e| crate::FileshareError::Config(format!("Failed to save paired devices: {}", e)))?;

        info!("💾 Saved {} paired devices to config", settings_copy.security.paired_devices.len());
        Ok(())
    }

    pub async fn shutdown(self) {
        info!("🛑 Initiating shutdown...");
        let _ = self.shutdown_tx.send(());
    }
}