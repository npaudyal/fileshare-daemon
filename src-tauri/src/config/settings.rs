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
    pub streaming: StreamingSettings, // NEW
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecuritySettings {
    pub require_pairing: bool,
    pub encryption_enabled: bool,
    pub allowed_devices: Vec<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamingSettings {
    pub enable_adaptive_chunking: bool,
    pub max_memory_buffer_mb: usize,
    pub stream_buffer_size_kb: usize,
    pub progress_report_interval_ms: u64,
    pub enable_chunk_validation: bool,
    pub max_concurrent_chunk_reads: usize,
    pub enable_streaming_mode: bool,
}

impl Default for StreamingSettings {
    fn default() -> Self {
        Self {
            enable_adaptive_chunking: true,
            max_memory_buffer_mb: 100,
            stream_buffer_size_kb: 64,
            progress_report_interval_ms: 500,
            enable_chunk_validation: true,
            max_concurrent_chunk_reads: 3,
            enable_streaming_mode: true,
        }
    }
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
                chunk_size: 1024 * 1024, // 1MB default, but will be adaptive
                max_concurrent_transfers: 5,
                bandwidth_limit_mbps: None,
                temp_dir: None,
            },
            security: SecuritySettings {
                require_pairing: false,
                encryption_enabled: true,
                allowed_devices: Vec::new(),
            },
            streaming: StreamingSettings::default(), // NEW
        }
    }
}

impl Settings {
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

            // Ensure streaming settings exist (for migration from older configs)
            if settings.streaming.max_memory_buffer_mb == 0 {
                settings.streaming = StreamingSettings::default();
            }

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
