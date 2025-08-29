use crate::pairing::errors::PairingError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedDevice {
    pub device_id: Uuid,
    pub device_name: String,
    pub custom_name: Option<String>,
    pub device_type: String,
    pub platform: String,
    pub public_key: Vec<u8>,  // Ed25519 public key
    pub paired_at: u64,        // Unix timestamp
    pub last_seen: u64,        // Unix timestamp
    pub last_ip: String,
    pub version: String,
    pub trust_level: TrustLevel,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TrustLevel {
    Verified,  // Fully paired and verified
    Trusted,   // Paired but not recently verified
    Unknown,   // Not paired
}

impl PairedDevice {
    pub fn new(
        device_id: Uuid,
        device_name: String,
        device_type: String,
        platform: String,
        public_key: Vec<u8>,
        ip_address: String,
        version: String,
    ) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        
        Self {
            device_id,
            device_name,
            custom_name: None,
            device_type,
            platform,
            public_key,
            paired_at: now,
            last_seen: now,
            last_ip: ip_address,
            version,
            trust_level: TrustLevel::Verified,
        }
    }
    
    pub fn update_last_seen(&mut self, ip_address: String) {
        self.last_seen = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        self.last_ip = ip_address;
    }
    
    pub fn display_name(&self) -> &str {
        self.custom_name.as_deref().unwrap_or(&self.device_name)
    }
    
    pub fn is_online(&self) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        
        // Consider device online if seen within last 30 seconds
        (now - self.last_seen) < 30
    }
}

pub struct PairedDeviceStorage {
    devices: Arc<RwLock<HashMap<Uuid, PairedDevice>>>,
    blocked_devices: Arc<RwLock<Vec<Uuid>>>,
}

impl PairedDeviceStorage {
    pub fn new() -> Self {
        Self {
            devices: Arc::new(RwLock::new(HashMap::new())),
            blocked_devices: Arc::new(RwLock::new(Vec::new())),
        }
    }
    
    pub async fn add_paired_device(&self, device: PairedDevice) -> Result<(), PairingError> {
        let mut devices = self.devices.write().await;
        let device_id = device.device_id;
        
        if devices.contains_key(&device_id) {
            return Err(PairingError::AlreadyPaired);
        }
        
        info!("Adding paired device: {} ({})", device.device_name, device_id);
        devices.insert(device_id, device);
        Ok(())
    }
    
    pub async fn remove_paired_device(&self, device_id: Uuid) -> Result<(), PairingError> {
        let mut devices = self.devices.write().await;
        if devices.remove(&device_id).is_some() {
            info!("Removed paired device: {}", device_id);
            Ok(())
        } else {
            Err(PairingError::StorageError("Device not found".to_string()))
        }
    }
    
    pub async fn get_paired_device(&self, device_id: Uuid) -> Option<PairedDevice> {
        let devices = self.devices.read().await;
        devices.get(&device_id).cloned()
    }
    
    pub async fn get_all_paired_devices(&self) -> Vec<PairedDevice> {
        let devices = self.devices.read().await;
        devices.values().cloned().collect()
    }
    
    pub async fn is_device_paired(&self, device_id: Uuid) -> bool {
        let devices = self.devices.read().await;
        devices.contains_key(&device_id)
    }
    
    pub async fn update_device_last_seen(&self, device_id: Uuid, ip_address: String) -> Result<(), PairingError> {
        let mut devices = self.devices.write().await;
        if let Some(device) = devices.get_mut(&device_id) {
            device.update_last_seen(ip_address);
            debug!("Updated last seen for device: {}", device_id);
            Ok(())
        } else {
            Err(PairingError::StorageError("Device not found".to_string()))
        }
    }
    
    pub async fn rename_device(&self, device_id: Uuid, custom_name: String) -> Result<(), PairingError> {
        let mut devices = self.devices.write().await;
        if let Some(device) = devices.get_mut(&device_id) {
            device.custom_name = Some(custom_name.clone());
            info!("Renamed device {} to '{}'", device_id, custom_name);
            Ok(())
        } else {
            Err(PairingError::StorageError("Device not found".to_string()))
        }
    }
    
    pub async fn block_device(&self, device_id: Uuid) -> Result<(), PairingError> {
        // Remove from paired devices if exists
        let _ = self.remove_paired_device(device_id).await;
        
        // Add to blocked list
        let mut blocked = self.blocked_devices.write().await;
        if !blocked.contains(&device_id) {
            blocked.push(device_id);
            info!("Blocked device: {}", device_id);
        }
        Ok(())
    }
    
    pub async fn unblock_device(&self, device_id: Uuid) -> Result<(), PairingError> {
        let mut blocked = self.blocked_devices.write().await;
        blocked.retain(|&id| id != device_id);
        info!("Unblocked device: {}", device_id);
        Ok(())
    }
    
    pub async fn is_device_blocked(&self, device_id: Uuid) -> bool {
        let blocked = self.blocked_devices.read().await;
        blocked.contains(&device_id)
    }
    
    pub async fn get_blocked_devices(&self) -> Vec<Uuid> {
        let blocked = self.blocked_devices.read().await;
        blocked.clone()
    }
    
    pub async fn verify_device_signature(&self, device_id: Uuid, data: &[u8], signature: &[u8]) -> bool {
        if let Some(device) = self.get_paired_device(device_id).await {
            crate::pairing::crypto::PairingCrypto::verify_ed25519(&device.public_key, data, signature)
        } else {
            false
        }
    }
    
    /// Load paired devices from settings (to be called on startup)
    pub async fn load_from_settings(&self, paired_devices: Vec<PairedDevice>, blocked_devices: Vec<Uuid>) {
        let mut devices = self.devices.write().await;
        for device in paired_devices {
            devices.insert(device.device_id, device);
        }
        
        let mut blocked = self.blocked_devices.write().await;
        *blocked = blocked_devices;
        
        info!("Loaded {} paired devices and {} blocked devices from settings", 
              devices.len(), blocked.len());
    }
    
    /// Export paired devices to save in settings
    pub async fn export_to_settings(&self) -> (Vec<PairedDevice>, Vec<Uuid>) {
        let devices = self.devices.read().await;
        let blocked = self.blocked_devices.read().await;
        
        (devices.values().cloned().collect(), blocked.clone())
    }
}