use crate::{FileshareError, Result};
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
                service_name: "_fileshare._tcp.local.".to_string(),
                timeout_seconds: 30,
            },
            transfer: TransferSettings {
                chunk_size: 8 * 1024 * 1024, // 8MB default chunk size (OPTIMIZED for local networks)
                max_concurrent_transfers: 5,
                bandwidth_limit_mbps: None,
                temp_dir: None,
                adaptive_chunk_size: false, // Use fixed large chunks for local networks
                compression_enabled: false, // Disabled for local networks (CPU overhead not worth it)
                parallel_chunks: 4, // Reduced to prevent overwhelming (OPTIMIZED)
                resume_enabled: true,
            },
            security: SecuritySettings {
                require_pairing: false,
                encryption_enabled: true,
                allowed_devices: Vec::new(),
            },
        }
    }
}

impl Settings {
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

            let settings: Settings = toml::from_str(&content)
                .map_err(|e| FileshareError::Config(format!("Failed to parse config: {}", e)))?;

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
