use crate::{FileshareError, Result};
use rand::{thread_rng, Rng};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio::time;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

const PIN_LENGTH: usize = 6;
const PIN_LIFETIME_SECONDS: u64 = 120; // 2 minutes
const MAX_PIN_ATTEMPTS: u32 = 3;
const RATE_LIMIT_WINDOW_SECONDS: u64 = 300; // 5 minutes
const PIN_REFRESH_INTERVAL_SECONDS: u64 = 120; // 2 minutes

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingPin {
    pub code: String,
    pub generated_at: u64,
    pub expires_at: u64,
}

#[derive(Debug, Clone)]
pub struct PendingPair {
    pub device_id: Uuid,
    pub device_name: String,
    pub initiated_at: Instant,
    pub pin_hash: String,
    pub attempts: u32,
    pub last_attempt: Option<Instant>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedDevice {
    pub id: Uuid,
    pub name: String,
    pub paired_at: u64,
    pub trust_level: TrustLevel,
    pub metadata: DeviceMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TrustLevel {
    Trusted,
    Verified,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceMetadata {
    pub platform: Option<String>,
    pub app_version: String,
    pub last_connected: u64,
    pub total_transfers: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PairingStatus {
    Pending,
    InProgress,
    Success,
    Failed(String),
    RateLimited { retry_after_seconds: u64 },
}

pub struct PairingManager {
    current_pin: Arc<RwLock<PairingPin>>,
    pending_pairs: Arc<RwLock<HashMap<Uuid, PendingPair>>>,
    paired_devices: Arc<RwLock<HashMap<Uuid, PairedDevice>>>,
    rate_limit_tracker: Arc<RwLock<HashMap<Uuid, Vec<Instant>>>>,
    pin_refresh_handle: Option<JoinHandle<()>>,
}

impl PairingManager {
    pub fn new() -> Result<Self> {
        let initial_pin = Self::generate_pin()?;
        
        Ok(Self {
            current_pin: Arc::new(RwLock::new(initial_pin)),
            pending_pairs: Arc::new(RwLock::new(HashMap::new())),
            paired_devices: Arc::new(RwLock::new(HashMap::new())),
            rate_limit_tracker: Arc::new(RwLock::new(HashMap::new())),
            pin_refresh_handle: None,
        })
    }

    pub async fn start(&mut self) -> Result<()> {
        info!("🔐 Starting pairing manager with PIN refresh");
        
        // Start PIN refresh task
        let current_pin = self.current_pin.clone();
        
        let handle = tokio::spawn(async move {
            let mut interval = time::interval(Duration::from_secs(PIN_REFRESH_INTERVAL_SECONDS));
            
            loop {
                interval.tick().await;
                
                match Self::generate_pin() {
                    Ok(new_pin) => {
                        info!("🔄 Refreshing pairing PIN");
                        let mut pin = current_pin.write().await;
                        *pin = new_pin;
                    }
                    Err(e) => {
                        error!("❌ Failed to refresh PIN: {}", e);
                    }
                }
            }
        });
        
        self.pin_refresh_handle = Some(handle);
        
        info!("✅ Pairing manager started successfully");
        Ok(())
    }

    pub async fn stop(&mut self) {
        if let Some(handle) = self.pin_refresh_handle.take() {
            handle.abort();
            info!("🛑 Pairing manager stopped");
        }
    }

    fn generate_pin() -> Result<PairingPin> {
        let mut rng = thread_rng();
        let pin_digits: Vec<u8> = (0..PIN_LENGTH)
            .map(|_| rng.gen_range(0..10))
            .collect();
        
        let code = pin_digits
            .iter()
            .map(|d| d.to_string())
            .collect::<String>();
        
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        
        Ok(PairingPin {
            code,
            generated_at: now,
            expires_at: now + PIN_LIFETIME_SECONDS,
        })
    }

    pub async fn get_current_pin(&self) -> PairingPin {
        self.current_pin.read().await.clone()
    }

    pub async fn refresh_pin(&self) -> Result<PairingPin> {
        let new_pin = Self::generate_pin()?;
        let mut pin = self.current_pin.write().await;
        *pin = new_pin.clone();
        info!("🔄 PIN manually refreshed");
        Ok(new_pin)
    }

    pub async fn initiate_pairing(
        &self,
        device_id: Uuid,
        device_name: String,
        provided_pin: String,
    ) -> Result<PairingStatus> {
        // Check rate limiting
        if let Some(retry_after) = self.check_rate_limit(device_id).await {
            warn!("⚠️ Device {} is rate limited", device_id);
            return Ok(PairingStatus::RateLimited {
                retry_after_seconds: retry_after,
            });
        }

        // Validate PIN format
        if provided_pin.len() != PIN_LENGTH || !provided_pin.chars().all(|c| c.is_ascii_digit()) {
            self.record_failed_attempt(device_id).await;
            return Ok(PairingStatus::Failed("Invalid PIN format".to_string()));
        }

        // Get current PIN and check if it matches
        let current_pin = self.current_pin.read().await;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Check if PIN has expired
        if now > current_pin.expires_at {
            return Ok(PairingStatus::Failed("PIN has expired".to_string()));
        }

        // Verify PIN
        if provided_pin != current_pin.code {
            self.record_failed_attempt(device_id).await;
            
            // Check if device should be rate limited after this failure
            let attempts = self.get_attempt_count(device_id).await;
            if attempts >= MAX_PIN_ATTEMPTS {
                if let Some(retry_after) = self.check_rate_limit(device_id).await {
                    return Ok(PairingStatus::RateLimited {
                        retry_after_seconds: retry_after,
                    });
                }
            }
            
            return Ok(PairingStatus::Failed(format!(
                "Incorrect PIN. {} attempts remaining",
                MAX_PIN_ATTEMPTS - attempts
            )));
        }

        // PIN is correct, create pending pair
        let pin_hash = Self::hash_pin(&provided_pin);
        let pending_pair = PendingPair {
            device_id,
            device_name: device_name.clone(),
            initiated_at: Instant::now(),
            pin_hash,
            attempts: 0,
            last_attempt: None,
        };

        let mut pending = self.pending_pairs.write().await;
        pending.insert(device_id, pending_pair);

        info!("✅ Pairing initiated with device {} ({})", device_name, device_id);
        Ok(PairingStatus::InProgress)
    }

    pub async fn complete_pairing(
        &self,
        device_id: Uuid,
        device_name: String,
        platform: Option<String>,
    ) -> Result<()> {
        // Remove from pending
        let mut pending = self.pending_pairs.write().await;
        pending.remove(&device_id);

        // Add to paired devices
        let paired_device = PairedDevice {
            id: device_id,
            name: device_name.clone(),
            paired_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            trust_level: TrustLevel::Trusted,
            metadata: DeviceMetadata {
                platform,
                app_version: env!("CARGO_PKG_VERSION").to_string(),
                last_connected: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
                total_transfers: 0,
            },
        };

        let mut paired = self.paired_devices.write().await;
        paired.insert(device_id, paired_device);

        // Clear rate limiting for this device
        let mut rate_limiter = self.rate_limit_tracker.write().await;
        rate_limiter.remove(&device_id);

        info!("🎉 Pairing completed with device {} ({})", device_name, device_id);
        Ok(())
    }

    pub async fn verify_pairing_response(
        &self,
        device_id: Uuid,
        provided_pin_hash: &str,
    ) -> Result<bool> {
        let pending = self.pending_pairs.read().await;
        
        if let Some(pending_pair) = pending.get(&device_id) {
            // Check if pairing request hasn't expired (5 minute timeout)
            if pending_pair.initiated_at.elapsed() > Duration::from_secs(300) {
                return Ok(false);
            }
            
            // Verify PIN hash matches
            Ok(pending_pair.pin_hash == provided_pin_hash)
        } else {
            Ok(false)
        }
    }

    pub async fn is_device_paired(&self, device_id: Uuid) -> bool {
        let paired = self.paired_devices.read().await;
        paired.contains_key(&device_id)
    }

    pub async fn get_paired_devices(&self) -> Vec<PairedDevice> {
        let paired = self.paired_devices.read().await;
        paired.values().cloned().collect()
    }

    pub async fn unpair_device(&self, device_id: Uuid) -> Result<()> {
        let mut paired = self.paired_devices.write().await;
        if paired.remove(&device_id).is_some() {
            info!("🔓 Device {} unpaired", device_id);
            Ok(())
        } else {
            Err(FileshareError::Unknown(format!(
                "Device {} is not paired",
                device_id
            )))
        }
    }

    pub async fn get_pairing_status(&self, device_id: Uuid) -> PairingStatus {
        // Check if already paired
        if self.is_device_paired(device_id).await {
            return PairingStatus::Success;
        }

        // Check if pairing is pending
        let pending = self.pending_pairs.read().await;
        if pending.contains_key(&device_id) {
            return PairingStatus::InProgress;
        }

        // Check rate limiting
        if let Some(retry_after) = self.check_rate_limit(device_id).await {
            return PairingStatus::RateLimited {
                retry_after_seconds: retry_after,
            };
        }

        PairingStatus::Pending
    }

    async fn check_rate_limit(&self, device_id: Uuid) -> Option<u64> {
        let mut tracker = self.rate_limit_tracker.write().await;
        let now = Instant::now();
        
        // Clean up old attempts
        if let Some(attempts) = tracker.get_mut(&device_id) {
            attempts.retain(|attempt| {
                now.duration_since(*attempt).as_secs() < RATE_LIMIT_WINDOW_SECONDS
            });
            
            if attempts.len() >= MAX_PIN_ATTEMPTS as usize {
                // Calculate time until oldest attempt expires
                if let Some(oldest) = attempts.first() {
                    let elapsed = now.duration_since(*oldest).as_secs();
                    if elapsed < RATE_LIMIT_WINDOW_SECONDS {
                        return Some(RATE_LIMIT_WINDOW_SECONDS - elapsed);
                    }
                }
            }
        }
        
        None
    }

    async fn record_failed_attempt(&self, device_id: Uuid) {
        let mut tracker = self.rate_limit_tracker.write().await;
        let attempts = tracker.entry(device_id).or_insert_with(Vec::new);
        attempts.push(Instant::now());
        
        debug!("📝 Recorded failed attempt for device {}", device_id);
    }

    async fn get_attempt_count(&self, device_id: Uuid) -> u32 {
        let tracker = self.rate_limit_tracker.read().await;
        let now = Instant::now();
        
        if let Some(attempts) = tracker.get(&device_id) {
            let recent_attempts = attempts
                .iter()
                .filter(|attempt| {
                    now.duration_since(**attempt).as_secs() < RATE_LIMIT_WINDOW_SECONDS
                })
                .count();
            recent_attempts as u32
        } else {
            0
        }
    }

    fn hash_pin(pin: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(pin);
        format!("{:x}", hasher.finalize())
    }

    pub async fn load_paired_devices(&mut self, devices: Vec<PairedDevice>) {
        let mut paired = self.paired_devices.write().await;
        for device in devices {
            paired.insert(device.id, device);
        }
        info!("📥 Loaded {} paired devices from storage", paired.len());
    }

    pub async fn export_paired_devices(&self) -> Vec<PairedDevice> {
        let paired = self.paired_devices.read().await;
        paired.values().cloned().collect()
    }
}