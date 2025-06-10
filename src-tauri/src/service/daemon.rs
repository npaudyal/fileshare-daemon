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

        // Clean up any previous instances first
        if let Err(e) = Self::cleanup_previous_instances().await {
            warn!("Failed to cleanup previous instances: {}", e);
        }

        // Initialize peer manager
        let peer_manager = Arc::new(RwLock::new(PeerManager::new(settings.clone()).await?));

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
            hotkey_manager,
            clipboard,
            shutdown_tx,
            shutdown_rx,
        })
    }

    // Cleanup function for previous instances and hotkeys
    // Fixed cleanup function in daemon.rs
    async fn cleanup_previous_instances() -> Result<()> {
        info!("🧹 Cleaning up previous instances and hotkeys...");

        #[cfg(target_os = "windows")]
        {
            use std::process::Command;

            // Get current process ID to avoid killing ourselves
            let current_pid = std::process::id();
            info!("🔍 Current process PID: {}", current_pid);

            // First, let's see what processes are running
            let list_result = Command::new("tasklist")
                .args(&["/FI", "IMAGENAME eq fileshare-daemon.exe", "/FO", "CSV"])
                .output();

            if let Ok(output) = list_result {
                let output_str = String::from_utf8_lossy(&output.stdout);
                info!("📋 Found processes:\n{}", output_str);

                // Parse the output to get PIDs and kill only other instances
                for line in output_str.lines().skip(1) {
                    // Skip header
                    if line.contains("fileshare-daemon.exe") {
                        // Parse CSV format: "Image Name","PID","Session Name","Session#","Mem Usage"
                        let parts: Vec<&str> = line.split(',').collect();
                        if parts.len() >= 2 {
                            if let Ok(pid) = parts[1].trim_matches('"').parse::<u32>() {
                                if pid != current_pid {
                                    info!("🎯 Killing previous instance with PID: {}", pid);
                                    let kill_result = Command::new("taskkill")
                                        .args(&["/F", "/PID", &pid.to_string()])
                                        .output();

                                    match kill_result {
                                        Ok(kill_output) => {
                                            if kill_output.status.success() {
                                                info!("✅ Successfully killed PID {}", pid);
                                            } else {
                                                let stderr =
                                                    String::from_utf8_lossy(&kill_output.stderr);
                                                warn!("⚠️ Failed to kill PID {}: {}", pid, stderr);
                                            }
                                        }
                                        Err(e) => {
                                            warn!("❌ Error killing PID {}: {}", pid, e);
                                        }
                                    }
                                } else {
                                    info!("⏭️ Skipping current process PID: {}", pid);
                                }
                            }
                        }
                    }
                }
            } else {
                info!("ℹ️ No previous instances found or tasklist failed");
            }

            // Wait for processes to fully terminate
            tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;

            // Try to unregister any lingering global hotkeys by creating and destroying a manager
            // This is safe and won't affect the current process
            if let Ok(temp_manager) = global_hotkey::GlobalHotKeyManager::new() {
                info!("🧹 Created temporary hotkey manager for cleanup");
                // The manager will automatically clean up on drop
                drop(temp_manager);
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            }
        }

        #[cfg(target_os = "macos")]
        {
            use std::process::Command;

            // Get current process ID to avoid killing ourselves
            let current_pid = std::process::id();

            // List processes and kill only others
            let list_result = Command::new("pgrep")
                .args(&["-f", "fileshare-daemon"])
                .output();

            if let Ok(output) = list_result {
                let output_str = String::from_utf8_lossy(&output.stdout);
                for line in output_str.lines() {
                    if let Ok(pid) = line.trim().parse::<u32>() {
                        if pid != current_pid {
                            info!("🎯 Killing previous instance with PID: {}", pid);
                            let _ = Command::new("kill")
                                .args(&["-9", &pid.to_string()])
                                .output();
                        }
                    }
                }
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
        }

        #[cfg(target_os = "linux")]
        {
            use std::process::Command;

            // Get current process ID to avoid killing ourselves
            let current_pid = std::process::id();

            // List processes and kill only others
            let list_result = Command::new("pgrep")
                .args(&["-f", "fileshare-daemon"])
                .output();

            if let Ok(output) = list_result {
                let output_str = String::from_utf8_lossy(&output.stdout);
                for line in output_str.lines() {
                    if let Ok(pid) = line.trim().parse::<u32>() {
                        if pid != current_pid {
                            info!("🎯 Killing previous instance with PID: {}", pid);
                            let _ = Command::new("kill")
                                .args(&["-9", &pid.to_string()])
                                .output();
                        }
                    }
                }
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
        }

        info!("✅ Cleanup completed - current process preserved");
        Ok(())
    }

    // Add method to get the hotkey event sender
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

        // Add a delay to ensure Tauri is fully initialized
        tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
        info!("⏱️ Tauri initialization delay completed");

        // Start hotkey manager with enhanced error handling
        match self.hotkey_manager.start().await {
            Ok(()) => {
                info!("✅ Hotkey manager started successfully");
            }
            Err(e) => {
                error!("❌ Failed to start hotkey manager: {}", e);
                warn!("⚠️ Continuing without hotkeys - basic functionality will still work");

                // Try one more time with a fresh manager after cleanup
                info!("🔄 Attempting to restart hotkey manager after additional cleanup...");

                if let Err(cleanup_err) = Self::cleanup_previous_instances().await {
                    warn!("Additional cleanup failed: {}", cleanup_err);
                }

                // Wait a bit longer and try again
                tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;

                // Create a new hotkey manager
                match HotkeyManager::new() {
                    Ok(mut new_manager) => match new_manager.start().await {
                        Ok(()) => {
                            info!("✅ Hotkey manager restarted successfully on second attempt");
                            self.hotkey_manager = new_manager;
                        }
                        Err(e2) => {
                            error!("❌ Failed to restart hotkey manager: {}", e2);
                            warn!("⚠️ Proceeding without hotkeys");
                        }
                    },
                    Err(e3) => {
                        error!("❌ Failed to create new hotkey manager: {}", e3);
                    }
                }
            }
        }

        // Additional delay to ensure hotkeys are properly registered
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        // Start discovery service
        let mut discovery_handle = if let Some(mut discovery) = self.discovery.take() {
            Some(tokio::spawn(async move {
                info!("🔍 Starting discovery service...");
                if let Err(e) = discovery.run().await {
                    error!("❌ Discovery service error: {}", e);
                } else {
                    info!("✅ Discovery service started successfully");
                }
            }))
        } else {
            error!("❌ Discovery service not available");
            return Err(crate::FileshareError::Unknown(
                "Discovery service not available".to_string(),
            ));
        };

        // Start peer manager
        let mut peer_manager_handle = {
            let peer_manager = self.peer_manager.clone();
            let settings = self.settings.clone();
            let clipboard = self.clipboard.clone();
            let mut shutdown_rx = self.shutdown_tx.subscribe();

            Some(tokio::spawn(async move {
                tokio::select! {
                    result = Self::run_peer_manager(peer_manager, settings, clipboard) => {
                        if let Err(e) = result {
                            error!("❌ Peer manager error: {}", e);
                        }
                    }
                    _ = shutdown_rx.recv() => {
                        info!("🛑 Peer manager shutdown requested");
                    }
                }
            }))
        };

        // Start hotkey event handler
        let mut hotkey_handle = {
            let peer_manager = self.peer_manager.clone();
            let clipboard = self.clipboard.clone();
            let mut hotkey_manager = self.hotkey_manager;
            let mut shutdown_rx = self.shutdown_tx.subscribe();

            Some(tokio::spawn(async move {
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
            }))
        };

        info!("✅ All background services started successfully");

        // Test hotkeys after a short delay
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
            info!("🎹 Hotkey system ready!");
            info!("🎹 Try copying a file with your hotkeys:");

            #[cfg(target_os = "windows")]
            {
                info!("   📋 Copy: Check logs above for the registered combination");
                info!("   📁 Paste: Check logs above for the registered combination");
                info!("   💡 If hotkeys don't work, try restarting the application");
            }

            #[cfg(target_os = "macos")]
            {
                info!("   📋 Copy: Cmd+Shift+Y");
                info!("   📁 Paste: Cmd+Shift+I");
            }

            #[cfg(target_os = "linux")]
            {
                info!("   📋 Copy: Ctrl+Shift+Y");
                info!("   📁 Paste: Ctrl+Shift+I");
            }
        });

        // Wait for shutdown signal or critical service failure
        let mut shutdown_rx = self.shutdown_rx;

        loop {
            tokio::select! {
                // Wait for explicit shutdown
                _ = shutdown_rx.recv() => {
                    info!("🛑 Shutdown signal received");
                    break;
                }

                // Monitor discovery service
                result = async {
                    if let Some(handle) = &mut discovery_handle {
                        handle.await
                    } else {
                        // If no handle, wait forever
                        std::future::pending().await
                    }
                } => {
                    match result {
                        Ok(_) => warn!("🔍 Discovery service completed unexpectedly"),
                        Err(e) => error!("❌ Discovery service panicked: {}", e),
                    }
                    discovery_handle = None; // Mark as completed
                }

                // Monitor peer manager
                result = async {
                    if let Some(handle) = &mut peer_manager_handle {
                        handle.await
                    } else {
                        // If no handle, wait forever
                        std::future::pending().await
                    }
                } => {
                    match result {
                        Ok(_) => warn!("🌐 Peer manager completed unexpectedly"),
                        Err(e) => error!("❌ Peer manager panicked: {}", e),
                    }
                    peer_manager_handle = None; // Mark as completed
                }

                // Monitor hotkey handler
                result = async {
                    if let Some(handle) = &mut hotkey_handle {
                        handle.await
                    } else {
                        // If no handle, wait forever
                        std::future::pending().await
                    }
                } => {
                    match result {
                        Ok(_) => warn!("🎹 Hotkey handler completed unexpectedly"),
                        Err(e) => error!("❌ Hotkey handler panicked: {}", e),
                    }
                    hotkey_handle = None; // Mark as completed
                }
            }
        }

        // Clean shutdown
        info!("🛑 Shutting down services...");

        // Send shutdown signal to all services
        if let Err(e) = self.shutdown_tx.send(()) {
            warn!("Failed to send shutdown signal: {}", e);
        }

        // Give services time to shut down gracefully
        tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;

        // Force abort remaining tasks
        if let Some(handle) = discovery_handle.take() {
            handle.abort();
        }
        if let Some(handle) = peer_manager_handle.take() {
            handle.abort();
        }
        if let Some(handle) = hotkey_handle.take() {
            handle.abort();
        }

        // Wait a bit more for cleanup
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        info!("✅ Fileshare Daemon stopped cleanly");
        Ok(())
    }

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

        let message_pm = peer_manager.clone();
        let message_clipboard = clipboard.clone();
        let message_handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(100));
            loop {
                interval.tick().await;

                let mut pm = message_pm.write().await;

                while let Ok((peer_id, message)) = pm.message_rx.try_recv() {
                    // Route outgoing transfer messages directly
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
