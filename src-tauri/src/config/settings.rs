use crate::{FileshareError, Result};
use crate::pairing::storage::{PairedDevice, TrustLevel};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub device: DeviceSettings,
    pub network: NetworkSettings,
    pub transfer: TransferSettings,
    pub security: SecuritySettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceSettings {
    pub id: Uuid,
    pub name: String,
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkSettings {
    pub port: u16,
    pub discovery_port: u16,
    pub http_port: u16,
    pub service_name: String,
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferSettings {
    pub chunk_size: usize,
    pub max_concurrent_transfers: usize,
    pub bandwidth_limit_mbps: Option<u32>,
    pub temp_dir: Option<PathBuf>,
    pub adaptive_chunk_size: bool,
    pub compression_enabled: bool,
    pub parallel_chunks: usize,
    pub resume_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecuritySettings {
    pub require_pairing: bool,
    pub encryption_enabled: bool,
    #[serde(default)]
    pub paired_devices: Vec<PairedDevice>,
    #[serde(default)]
    pub blocked_devices: Vec<Uuid>,
    // Keep for backward compatibility, will be migrated
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_devices: Vec<Uuid>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            device: DeviceSettings {
                id: Uuid::new_v4(),
                name: gethostname::gethostname().to_string_lossy().to_string(),
                icon: None,
            },
            network: NetworkSettings {
                port: 9876,
                discovery_port: 9877,
                http_port: 9878,
                service_name: "_fileshare._tcp.local.".to_string(),
                timeout_seconds: 30,
            },
            transfer: TransferSettings {
                chunk_size: 1024 * 1024, // 1MB default chunk size
                max_concurrent_transfers: 5,
                bandwidth_limit_mbps: None,
                temp_dir: None,
                adaptive_chunk_size: true,
                compression_enabled: false,
                parallel_chunks: 8, // OPTIMIZED: Increased from 4
                resume_enabled: true,
            },
            security: SecuritySettings {
                require_pairing: false,
                encryption_enabled: true,
                paired_devices: Vec::new(),
                blocked_devices: Vec::new(),
                allowed_devices: Vec::new(),
            },
        }
    }
}

impl Settings {
    /// Migrate from old allowed_devices to new paired_devices structure
    pub fn migrate_allowed_devices(&mut self) {
        if !self.security.allowed_devices.is_empty() {
            // Convert each allowed device to a PairedDevice with minimal info
            for device_id in self.security.allowed_devices.drain(..) {
                // Check if already migrated
                if !self.security.paired_devices.iter().any(|d| d.device_id == device_id) {
                    let paired_device = PairedDevice::new(
                        device_id,
                        format!("Device-{}", device_id.to_string().split('-').next().unwrap_or("unknown")),
                        "unknown".to_string(),
                        "unknown".to_string(),
                        Vec::new(), // Empty public key, will be updated on next pairing
                        "0.0.0.0".to_string(),
                        "unknown".to_string(),
                    );
                    self.security.paired_devices.push(paired_device);
                }
            }
            // Clear allowed_devices after migration
            self.security.allowed_devices.clear();
        }
    }
    
    pub fn validate_transfer_settings(&self) -> Result<()> {
        let transfer = &self.transfer;

        // Validate chunk size (64KB to 16MB)
        if transfer.chunk_size < 64 * 1024 {
            return Err(FileshareError::Config(
                "Chunk size must be at least 64KB".to_string(),
            ));
        }
        if transfer.chunk_size > 16 * 1024 * 1024 {
            return Err(FileshareError::Config(
                "Chunk size must not exceed 16MB".to_string(),
            ));
        }

        // Validate max concurrent transfers (1-10)
        if transfer.max_concurrent_transfers == 0 || transfer.max_concurrent_transfers > 10 {
            return Err(FileshareError::Config(
                "Max concurrent transfers must be between 1 and 10".to_string(),
            ));
        }

        // Validate parallel chunks (1-16)
        if transfer.parallel_chunks == 0 || transfer.parallel_chunks > 16 {
            return Err(FileshareError::Config(
                "Parallel chunks must be between 1 and 16".to_string(),
            ));
        }

        Ok(())
    }
    pub fn load(config_path: Option<&str>) -> Result<Self> {
        let path = match config_path {
            Some(path) => PathBuf::from(path),
            None => Self::default_config_path()?,
        };

        if path.exists() {
            let content = fs::read_to_string(&path)
                .map_err(|e| FileshareError::Config(format!("Failed to read config: {}", e)))?;

            let mut settings: Settings = toml::from_str(&content)
                .map_err(|e| FileshareError::Config(format!("Failed to parse config: {}", e)))?;

            // Migrate old allowed_devices to paired_devices if needed
            settings.migrate_allowed_devices();
            
            // Validate settings before returning
            settings.validate_transfer_settings()?;

            Ok(settings)
        } else {
            let settings = Self::default();
            settings.save(Some(&path))?;
            Ok(settings)
        }
    }

    pub fn save(&self, config_path: Option<&Path>) -> Result<()> {
        let path = match config_path {
            Some(path) => path.to_path_buf(),
            None => Self::default_config_path()?,
        };

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                FileshareError::Config(format!("Failed to create config dir: {}", e))
            })?;
        }

        let content = toml::to_string_pretty(self)
            .map_err(|e| FileshareError::Config(format!("Failed to serialize config: {}", e)))?;

        fs::write(&path, content)
            .map_err(|e| FileshareError::Config(format!("Failed to write config: {}", e)))?;

        Ok(())
    }

    fn default_config_path() -> Result<PathBuf> {
        let proj_dirs = ProjectDirs::from("com", "fileshare", "daemon").ok_or_else(|| {
            FileshareError::Config("Failed to get project directories".to_string())
        })?;

        Ok(proj_dirs.config_dir().join("config.toml"))
    }

    pub fn get_bind_address(&self) -> SocketAddr {
        format!("0.0.0.0:{}", self.network.port).parse().unwrap()
    }
}
