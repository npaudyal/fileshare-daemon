use crate::{config::Settings, network::peer_quic::PeerManager, Result};
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
    pub last_seen: u64,
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

        // Start broadcaster
        let broadcast_settings = settings.clone();
        tokio::spawn(async move {
            Self::run_broadcaster(broadcast_settings).await;
        });

        // Start listener
        let listen_settings = settings.clone();
        let listen_discovered_devices = discovered_devices.clone();
        let listen_peer_manager = peer_manager.clone();
        tokio::spawn(async move {
            Self::run_listener(
                listen_settings,
                listen_discovered_devices,
                listen_peer_manager,
            )
            .await;
        });

        // Start cleanup task
        let cleanup_discovered_devices = discovered_devices.clone();
        tokio::spawn(async move {
            Self::run_cleanup(cleanup_discovered_devices).await;
        });

        info!("Discovery service started successfully");
        Ok(())
    }

    async fn run_broadcaster(settings: Arc<Settings>) {
        info!(
            "🔊 Starting discovery broadcaster on port {}",
            settings.network.discovery_port
        );

        // Add a small delay to ensure network is ready
        tokio::time::sleep(Duration::from_secs(1)).await;

        loop {
            match Self::broadcast_presence(&settings).await {
                Ok(()) => {
                    debug!("✅ Successfully broadcasted presence");
                }
                Err(e) => {
                    warn!("❌ Failed to broadcast presence: {}", e);
                }
            }

            // Wait 5 seconds before next broadcast (reduced for testing)
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }

    async fn run_listener(
        settings: Arc<Settings>,
        discovered_devices: Arc<RwLock<HashMap<Uuid, DeviceInfo>>>,
        peer_manager: Arc<RwLock<PeerManager>>,
    ) {
        info!(
            "👂 Starting discovery listener on port {}",
            settings.network.discovery_port
        );

        // Create listener socket
        let bind_addr = format!("0.0.0.0:{}", settings.network.discovery_port);
        info!("🔌 Binding discovery listener to {}", bind_addr);

        let socket = match tokio::net::UdpSocket::bind(&bind_addr).await {
            Ok(socket) => {
                info!("✅ Discovery listener bound successfully to {}", bind_addr);
                socket
            }
            Err(e) => {
                error!(
                    "❌ Failed to bind discovery listener to {}: {}",
                    bind_addr, e
                );
                return;
            }
        };

        let mut buf = [0; 2048];

        info!("👂 Discovery listener ready, waiting for packets...");

        loop {
            match socket.recv_from(&mut buf).await {
                Ok((len, addr)) => {
                    info!("📦 Received discovery packet from {} ({} bytes)", addr, len);

                    let data = String::from_utf8_lossy(&buf[..len]);
                    debug!("Discovery packet content: {}", data);

                    if let Err(e) = Self::handle_discovery_packet(
                        &data,
                        addr,
                        &settings,
                        discovered_devices.clone(),
                        peer_manager.clone(),
                    )
                    .await
                    {
                        warn!("Failed to handle discovery packet from {}: {}", addr, e);
                    }
                }
                Err(e) => {
                    warn!("Discovery listener error: {}", e);
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }

    async fn broadcast_presence(settings: &Settings) -> Result<()> {
        use tokio::net::UdpSocket;

        debug!("🔊 Broadcasting presence for device {} on port {}...", 
               settings.device.name, settings.network.port);

        // Create broadcast socket
        let socket = UdpSocket::bind("0.0.0.0:0")
            .await
            .map_err(|e| crate::FileshareError::Network(e))?;

        socket
            .set_broadcast(true)
            .map_err(|e| crate::FileshareError::Network(e))?;

        // Create announcement
        let announcement = serde_json::json!({
            "device_id": settings.device.id,
            "device_name": settings.device.name,
            "port": settings.network.port,
            "version": env!("CARGO_PKG_VERSION"),
            "timestamp": SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
        });

        let data = announcement.to_string();
        let broadcast_addr = format!("255.255.255.255:{}", settings.network.discovery_port);

        debug!(
            "Sending broadcast to {} with data: {}",
            broadcast_addr, data
        );

        match socket.send_to(data.as_bytes(), &broadcast_addr).await {
            Ok(bytes_sent) => {
                debug!(
                    "Successfully broadcasted {} bytes to {}",
                    bytes_sent, broadcast_addr
                );
                Ok(())
            }
            Err(e) => {
                warn!("Failed to send broadcast to {}: {}", broadcast_addr, e);
                Err(crate::FileshareError::Network(e))
            }
        }
    }

    async fn handle_discovery_packet(
        data: &str,
        addr: SocketAddr,
        settings: &Settings,
        discovered_devices: Arc<RwLock<HashMap<Uuid, DeviceInfo>>>,
        peer_manager: Arc<RwLock<PeerManager>>,
    ) -> Result<()> {
        use serde_json::Value;

        debug!("Parsing discovery packet from {}", addr);

        let announcement: Value = serde_json::from_str(data)
            .map_err(|e| crate::FileshareError::Unknown(format!("JSON parse error: {}", e)))?;

        let device_id_str = announcement
            .get("device_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| crate::FileshareError::Discovery("Missing device_id".to_string()))?;

        let device_id = Uuid::parse_str(device_id_str)
            .map_err(|e| crate::FileshareError::Discovery(format!("Invalid device_id: {}", e)))?;

        // Don't discover ourselves
        if device_id == settings.device.id {
            debug!("Ignoring self-discovery from {}", addr);
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

        // Create device address using the actual IP from the packet
        let device_addr = SocketAddr::new(addr.ip(), port);

        let device_info = DeviceInfo {
            id: device_id,
            name: device_name.clone(),
            addr: device_addr,
            last_seen: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            version,
        };

        info!(
            "🎯 Discovered device: {} ({}) at {} - attempting QUIC connection",
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
                error!("❌ Failed to notify peer manager about discovered device: {}", e);
            } else {
                info!("✅ Successfully notified peer manager about device discovery");
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
