use crate::{FileshareError, Result};
use ed25519_dalek::{SigningKey, VerifyingKey, Signature, Signer, Verifier};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

#[derive(Clone)]
pub struct DeviceKeypair {
    signing_key: SigningKey,
    verifying_key: VerifyingKey,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceKeyData {
    pub public_key: String,  // Base64 encoded
    pub private_key: String,  // Base64 encoded
}

impl DeviceKeypair {
    /// Generate a new device keypair
    pub fn generate() -> Result<Self> {
        let mut csprng = OsRng;
        // Generate 32 random bytes for the private key
        let mut secret_bytes = [0u8; 32];
        use rand::RngCore;
        csprng.fill_bytes(&mut secret_bytes);
        
        let signing_key = SigningKey::from_bytes(&secret_bytes);
        let verifying_key = signing_key.verifying_key();
        
        Ok(Self { signing_key, verifying_key })
    }

    /// Load keypair from data
    pub fn from_data(data: &DeviceKeyData) -> Result<Self> {
        let private_bytes = BASE64.decode(&data.private_key)
            .map_err(|e| FileshareError::Crypto(format!("Failed to decode private key: {}", e)))?;

        if private_bytes.len() != 32 {
            return Err(FileshareError::Crypto("Invalid private key length".to_string()));
        }

        let mut key_bytes = [0u8; 32];
        key_bytes.copy_from_slice(&private_bytes);
        
        let signing_key = SigningKey::from_bytes(&key_bytes);
        let verifying_key = signing_key.verifying_key();

        Ok(Self { signing_key, verifying_key })
    }

    /// Export keypair as data
    pub fn to_data(&self) -> DeviceKeyData {
        DeviceKeyData {
            public_key: BASE64.encode(self.verifying_key.as_bytes()),
            private_key: BASE64.encode(self.signing_key.as_bytes()),
        }
    }

    /// Get public key bytes
    pub fn public_key_bytes(&self) -> Vec<u8> {
        self.verifying_key.as_bytes().to_vec()
    }

    /// Get public key as base64 string
    pub fn public_key_base64(&self) -> String {
        BASE64.encode(self.verifying_key.as_bytes())
    }

    /// Sign a message
    pub fn sign(&self, message: &[u8]) -> Vec<u8> {
        let signature = self.signing_key.sign(message);
        signature.to_bytes().to_vec()
    }

    /// Verify a signature
    pub fn verify(&self, message: &[u8], signature: &[u8]) -> bool {
        if signature.len() != 64 {
            return false;
        }
        
        let mut sig_bytes = [0u8; 64];
        sig_bytes.copy_from_slice(signature);
        let sig = Signature::from_bytes(&sig_bytes);
        
        self.verifying_key.verify(message, &sig).is_ok()
    }

    /// Verify a signature from another device's public key
    pub fn verify_from_public_key(
        public_key_bytes: &[u8],
        message: &[u8],
        signature: &[u8],
    ) -> bool {
        if public_key_bytes.len() != 32 || signature.len() != 64 {
            return false;
        }
        
        let mut pub_key_array = [0u8; 32];
        pub_key_array.copy_from_slice(public_key_bytes);
        
        let mut sig_array = [0u8; 64];
        sig_array.copy_from_slice(signature);
        
        if let Ok(public_key) = VerifyingKey::from_bytes(&pub_key_array) {
            let sig = Signature::from_bytes(&sig_array);
            return public_key.verify(message, &sig).is_ok();
        }
        false
    }

    /// Save keypair to file (encrypted in production)
    pub fn save_to_file(&self, path: &PathBuf) -> Result<()> {
        let data = self.to_data();
        let json = serde_json::to_string_pretty(&data)
            .map_err(|e| FileshareError::Crypto(format!("Failed to serialize keypair: {}", e)))?;

        // Ensure directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| FileshareError::Crypto(format!("Failed to create directory: {}", e)))?;
        }

        // TODO: In production, encrypt this file
        fs::write(path, json)
            .map_err(|e| FileshareError::Crypto(format!("Failed to save keypair: {}", e)))?;

        Ok(())
    }

    /// Load keypair from file
    pub fn load_from_file(path: &PathBuf) -> Result<Self> {
        if !path.exists() {
            return Err(FileshareError::Crypto("Keypair file not found".to_string()));
        }

        let json = fs::read_to_string(path)
            .map_err(|e| FileshareError::Crypto(format!("Failed to read keypair: {}", e)))?;

        let data: DeviceKeyData = serde_json::from_str(&json)
            .map_err(|e| FileshareError::Crypto(format!("Failed to parse keypair: {}", e)))?;

        Self::from_data(&data)
    }

    /// Load or generate keypair
    pub fn load_or_generate(path: &PathBuf) -> Result<Self> {
        if path.exists() {
            Self::load_from_file(path)
        } else {
            let keypair = Self::generate()?;
            keypair.save_to_file(path)?;
            Ok(keypair)
        }
    }
}

/// Create a signed challenge for pairing
pub fn create_pairing_challenge(pin: &str, device_id: &str, keypair: &DeviceKeypair) -> Vec<u8> {
    let challenge = format!("{}:{}", pin, device_id);
    keypair.sign(challenge.as_bytes())
}

/// Verify a pairing challenge
pub fn verify_pairing_challenge(
    pin: &str,
    device_id: &str,
    signature: &[u8],
    public_key: &[u8],
) -> bool {
    let challenge = format!("{}:{}", pin, device_id);
    DeviceKeypair::verify_from_public_key(public_key, challenge.as_bytes(), signature)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keypair_generation() {
        let keypair = DeviceKeypair::generate().unwrap();
        assert_eq!(keypair.public_key_bytes().len(), 32);
    }

    #[test]
    fn test_signing_and_verification() {
        let keypair = DeviceKeypair::generate().unwrap();
        let message = b"test message";
        
        let signature = keypair.sign(message);
        assert!(keypair.verify(message, &signature));
        
        // Wrong message should fail
        assert!(!keypair.verify(b"wrong message", &signature));
    }

    #[test]
    fn test_keypair_serialization() {
        let keypair1 = DeviceKeypair::generate().unwrap();
        let data = keypair1.to_data();
        let keypair2 = DeviceKeypair::from_data(&data).unwrap();
        
        // Both keypairs should produce same signatures
        let message = b"test";
        let sig1 = keypair1.sign(message);
        let sig2 = keypair2.sign(message);
        
        assert_eq!(sig1, sig2);
    }

    #[test]
    fn test_pairing_challenge() {
        let keypair = DeviceKeypair::generate().unwrap();
        let pin = "1234";
        let device_id = "test-device";
        
        let signature = create_pairing_challenge(pin, device_id, &keypair);
        
        assert!(verify_pairing_challenge(
            pin,
            device_id,
            &signature,
            &keypair.public_key_bytes()
        ));
        
        // Wrong PIN should fail
        assert!(!verify_pairing_challenge(
            "5678",
            device_id,
            &signature,
            &keypair.public_key_bytes()
        ));
    }
}