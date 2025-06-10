use crate::{
    config::Settings,
    network::{DiscoveryService, MessageType, PeerManager},
    service::file_transfer::TransferDirection,
    Result,
};
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tracing::{error, info, warn};

pub struct FileshareDaemon {
    settings: Arc<Settings>,
    pub discovery: Option<DiscoveryService>,
    pub peer_manager: Arc<RwLock<PeerManager>>,
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

        Ok(Self {
            settings,
            discovery: Some(discovery),
            peer_manager,
            shutdown_tx,
            shutdown_rx,
        })
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

        // Start peer manager
        let peer_manager_handle = {
            let peer_manager = self.peer_manager.clone();
            let settings = self.settings.clone();
            let mut shutdown_rx = self.shutdown_tx.subscribe();

            tokio::spawn(async move {
                tokio::select! {
                    result = Self::run_peer_manager(peer_manager, settings) => {
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

        info!("✅ All background services started successfully");

        // Wait for shutdown signal
        let mut shutdown_rx = self.shutdown_rx;
        shutdown_rx.recv().await.ok();

        // Clean shutdown
        info!("🛑 Shutting down services...");
        discovery_handle.abort();
        peer_manager_handle.abort();

        info!("✅ Fileshare Daemon stopped");
        Ok(())
    }

    async fn run_peer_manager(
        peer_manager: Arc<RwLock<PeerManager>>,
        settings: Arc<Settings>,
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
                    // Note: We removed clipboard from here since it's handled in main.rs now
                    if let Err(e) = pm.handle_message_without_clipboard(peer_id, message).await {
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
