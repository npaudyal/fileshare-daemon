use rand::Rng;
use sha2::{Digest, Sha256};

pub fn generate_device_key() -> [u8; 32] {
    let mut rng = rand::thread_rng();
    let mut key = [0u8; 32];
    rng.fill(&mut key);
    key
}

pub fn hash_data(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

pub fn verify_checksum(data: &[u8], expected: &str) -> bool {
    hash_data(data) == expected
}
