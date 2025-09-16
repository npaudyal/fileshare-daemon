use rand::Rng;
use std::sync::Arc;
use tokio::sync::RwLock;
use std::collections::HashSet;
use std::time::{Duration, Instant};

pub struct PinGenerator {
    recent_pins: Arc<RwLock<HashSet<String>>>,
    last_cleanup: Arc<RwLock<Instant>>,
}

impl PinGenerator {
    pub fn new() -> Self {
        Self {
            recent_pins: Arc::new(RwLock::new(HashSet::new())),
            last_cleanup: Arc::new(RwLock::new(Instant::now())),
        }
    }

    /// Generate a unique 4-digit PIN
    pub async fn generate_pin(&self) -> String {
        // Clean up old PINs every hour
        self.cleanup_old_pins().await;

        let pin = {
            use rand::RngCore;
            let mut rng = rand::rngs::OsRng;
            loop {
                // Generate 4-digit PIN (1000-9999)
                let pin_num = (rng.next_u32() % 9000) + 1000;
                let pin = pin_num.to_string();

                // Check uniqueness
                let recent = self.recent_pins.read().await;
                if !recent.contains(&pin) {
                    break pin;
                }
                // Drop the read lock before continuing the loop
                drop(recent);
            }
        };

        // Add to recent pins
        let mut recent = self.recent_pins.write().await;
        recent.insert(pin.clone());
        pin
    }

    /// Verify PIN with constant-time comparison
    pub fn verify_pin(pin1: &str, pin2: &str) -> bool {
        if pin1.len() != pin2.len() {
            return false;
        }

        // Constant-time comparison to prevent timing attacks
        let mut result = 0u8;
        for (a, b) in pin1.bytes().zip(pin2.bytes()) {
            result |= a ^ b;
        }
        result == 0
    }

    async fn cleanup_old_pins(&self) {
        let mut last_cleanup = self.last_cleanup.write().await;
        
        // Clean up every hour
        if last_cleanup.elapsed() > Duration::from_secs(3600) {
            let mut recent = self.recent_pins.write().await;
            recent.clear();
            *last_cleanup = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_pin_generation() {
        let generator = PinGenerator::new();
        let pin = generator.generate_pin().await;
        
        assert_eq!(pin.len(), 4);
        assert!(pin.parse::<u32>().is_ok());
        assert!(pin.parse::<u32>().unwrap() >= 1000);
        assert!(pin.parse::<u32>().unwrap() < 10000);
    }

    #[test]
    fn test_pin_verification() {
        assert!(PinGenerator::verify_pin("1234", "1234"));
        assert!(!PinGenerator::verify_pin("1234", "5678"));
        assert!(!PinGenerator::verify_pin("123", "1234"));
    }

    #[tokio::test]
    async fn test_pin_uniqueness() {
        let generator = PinGenerator::new();
        let mut pins = HashSet::new();
        
        // Generate 100 PINs and ensure they're unique
        for _ in 0..100 {
            let pin = generator.generate_pin().await;
            assert!(!pins.contains(&pin), "Generated duplicate PIN");
            pins.insert(pin);
        }
    }
}