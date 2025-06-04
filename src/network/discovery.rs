use crate::{config::Settings, network::peer::PeerManager, Result};
use serde::{Deserialize, Serialize};
use serde_json;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, RwLock};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub id: Uuid,
    pub name: String,
    pub addr: SocketAddr,
    pub last_seen: u64, // Use timestamp instead of Instant
    pub version: String,
}

#[derive(Clone)]
pub struct DiscoveryService {
    settings: Arc<Settings>,
    peer_manager: Arc<RwLock<PeerManager>>,
    discovered_devices: Arc<RwLock<HashMap<Uuid, DeviceInfo>>>,
}

impl DiscoveryService {
    pub async fn new(
        settings: Arc<Settings>,
        peer_manager: Arc<RwLock<PeerManager>>,
        _shutdown_rx: broadcast::Receiver<()>,
    ) -> Result<Self> {
        Ok(Self {
            settings,
            peer_manager,
            discovered_devices: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    pub async fn run(&mut self) -> Result<()> {
        info!("Starting discovery service");

        let settings = self.settings.clone();
        let discovered_devices = self.discovered_devices.clone();
        let peer_manager = self.peer_manager.clone();

        // Start discovery listener
        tokio::spawn(async move {
            if let Err(e) =
                Self::run_discovery_loop(settings, discovered_devices, peer_manager).await
            {
                error!("Discovery loop error: {}", e);
            }
        });

        // Start cleanup task
        let discovered_devices_cleanup = self.discovered_devices.clone();
        tokio::spawn(async move {
            Self::run_cleanup(discovered_devices_cleanup).await;
        });

        info!("Discovery service started successfully");
        Ok(())
    }

    async fn run_discovery_loop(
        settings: Arc<Settings>,
        discovered_devices: Arc<RwLock<HashMap<Uuid, DeviceInfo>>>,
        peer_manager: Arc<RwLock<PeerManager>>,
    ) -> Result<()> {
        info!("Starting discovery loop");

        loop {
            // Broadcast our presence
            if let Err(e) = Self::broadcast_presence(&settings).await {
                warn!("Failed to broadcast presence: {}", e);
            }

            // Listen for other devices
            if let Err(e) = Self::listen_for_broadcasts(
                &settings,
                discovered_devices.clone(),
                peer_manager.clone(),
            )
            .await
            {
                warn!("Failed to listen for broadcasts: {}", e);
            }

            // Wait before next cycle
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    }

    async fn broadcast_presence(settings: &Settings) -> Result<()> {
        use tokio::net::UdpSocket;

        let socket = UdpSocket::bind("0.0.0.0:0").await?;
        socket.set_broadcast(true)?;

        let announcement = serde_json::json!({
            "device_id": settings.device.id,
            "device_name": settings.device.name,
            "port": settings.network.port,
            "version": env!("CARGO_PKG_VERSION"),
            "timestamp": SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
        });

        let data = announcement.to_string();
        let broadcast_addr = format!("255.255.255.255:{}", settings.network.discovery_port);

        socket.send_to(data.as_bytes(), &broadcast_addr).await?;
        debug!("Broadcasted presence");

        Ok(())
    }

    async fn listen_for_broadcasts(
        settings: &Settings,
        discovered_devices: Arc<RwLock<HashMap<Uuid, DeviceInfo>>>,
        peer_manager: Arc<RwLock<PeerManager>>,
    ) -> Result<()> {
        use serde_json::Value;
        use tokio::net::UdpSocket;

        let bind_addr = format!("0.0.0.0:{}", settings.network.discovery_port);
        let socket = UdpSocket::bind(&bind_addr).await?;

        let mut buf = [0; 1024];

        // Use tokio timeout instead of set_recv_timeout
        let timeout_duration = Duration::from_secs(1);

        match tokio::time::timeout(timeout_duration, socket.recv_from(&mut buf)).await {
            Ok(Ok((len, addr))) => {
                let data = String::from_utf8_lossy(&buf[..len]);

                if let Ok(announcement) = serde_json::from_str::<Value>(&data) {
                    if let Some(device_id_str) =
                        announcement.get("device_id").and_then(|v| v.as_str())
                    {
                        if let Ok(device_id) = Uuid::parse_str(device_id_str) {
                            // Don't discover ourselves
                            if device_id == settings.device.id {
                                return Ok(());
                            }

                            let device_name = announcement
                                .get("device_name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("Unknown Device")
                                .to_string();

                            let port = announcement
                                .get("port")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(9876) as u16;

                            let version = announcement
                                .get("version")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown")
                                .to_string();

                            let device_info = DeviceInfo {
                                id: device_id,
                                name: device_name,
                                addr: SocketAddr::new(addr.ip(), port),
                                last_seen: SystemTime::now()
                                    .duration_since(UNIX_EPOCH)
                                    .unwrap()
                                    .as_secs(),
                                version,
                            };

                            info!(
                                "Discovered device: {} ({}) at {}",
                                device_info.name, device_info.id, device_info.addr
                            );

                            // Update discovered devices
                            {
                                let mut devices = discovered_devices.write().await;
                                devices.insert(device_id, device_info.clone());
                            }

                            // Notify peer manager
                            {
                                let mut pm = peer_manager.write().await;
                                if let Err(e) = pm.on_device_discovered(device_info).await {
                                    warn!("Failed to notify peer manager: {}", e);
                                }
                            }
                        }
                    }
                }
            }
            Ok(Err(e)) => {
                warn!("UDP receive error: {}", e);
            }
            Err(_) => {
                // Timeout - this is expected, continue
                debug!("Discovery listen timeout (normal behavior)");
            }
        }

        Ok(())
    }

    async fn run_cleanup(discovered_devices: Arc<RwLock<HashMap<Uuid, DeviceInfo>>>) {
        let mut interval = tokio::time::interval(Duration::from_secs(60));

        loop {
            interval.tick().await;

            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();
            let timeout_secs = 300; // 5 minutes timeout

            let mut devices = discovered_devices.write().await;
            let initial_count = devices.len();

            devices.retain(|_, device| now - device.last_seen < timeout_secs);

            let removed_count = initial_count - devices.len();
            if removed_count > 0 {
                debug!("Cleaned up {} stale devices", removed_count);
            }
        }
    }

    pub async fn get_discovered_devices(&self) -> Vec<DeviceInfo> {
        let devices = self.discovered_devices.read().await;
        devices.values().cloned().collect()
    }
}
