use crate::pairing::errors::PairingError;
use rand::{thread_rng, Rng};
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM};
use ring::agreement::{EphemeralPrivateKey, PublicKey, UnparsedPublicKey, X25519};
use ring::digest;
use ring::hkdf;
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{Ed25519KeyPair, KeyPair};
use subtle::ConstantTimeEq;

pub struct PairingCrypto;

impl PairingCrypto {
    /// Generate a secure 6-digit PIN
    pub fn generate_pin() -> String {
        let mut rng = thread_rng();
        let pin: u32 = rng.gen_range(100_000..1_000_000);
        format!("{:06}", pin)
    }
    
    /// Constant-time PIN comparison to prevent timing attacks
    pub fn verify_pin(provided: &str, expected: &str) -> bool {
        provided.as_bytes().ct_eq(expected.as_bytes()).into()
    }
    
    /// Generate ephemeral X25519 key pair for ECDH
    pub fn generate_ephemeral_keypair() -> Result<(EphemeralPrivateKey, Vec<u8>), PairingError> {
        let rng = SystemRandom::new();
        let private_key = EphemeralPrivateKey::generate(&X25519, &rng)
            .map_err(|e| PairingError::CryptoError(format!("Failed to generate ephemeral key: {:?}", e)))?;
        
        let public_key = private_key.compute_public_key()
            .map_err(|e| PairingError::CryptoError(format!("Failed to compute public key: {:?}", e)))?;
        
        Ok((private_key, public_key.as_ref().to_vec()))
    }
    
    /// For now, use the same ephemeral approach but generate fresh keys
    /// This is a temporary solution until we implement proper key storage
    pub fn generate_storable_keypair() -> Result<(Vec<u8>, Vec<u8>), PairingError> {
        // For now, just generate a regular keypair and return placeholder bytes
        let (_, public_key) = Self::generate_ephemeral_keypair()?;
        let placeholder_private = vec![0u8; 32]; // Placeholder - will be regenerated
        Ok((placeholder_private, public_key))
    }
    
    /// For now, fall back to regular ECDH (temporary solution)
    pub fn derive_shared_secret_from_raw(
        _private_key_bytes: &[u8],
        peer_public_key: &[u8],
    ) -> Result<Vec<u8>, PairingError> {
        // Temporary: generate fresh keys and use peer key
        let (private_key, _) = Self::generate_ephemeral_keypair()?;
        Self::derive_shared_secret(private_key, peer_public_key)
    }
    
    
    /// Generate long-term Ed25519 key pair for device authentication
    pub fn generate_device_keypair() -> Result<(Vec<u8>, Vec<u8>), PairingError> {
        let rng = SystemRandom::new();
        let pkcs8_bytes = Ed25519KeyPair::generate_pkcs8(&rng)
            .map_err(|e| PairingError::CryptoError(format!("Failed to generate Ed25519 key: {:?}", e)))?;
        
        let key_pair = Ed25519KeyPair::from_pkcs8(pkcs8_bytes.as_ref())
            .map_err(|e| PairingError::CryptoError(format!("Failed to parse Ed25519 key: {:?}", e)))?;
        
        let public_key = key_pair.public_key().as_ref().to_vec();
        let private_key = pkcs8_bytes.as_ref().to_vec();
        
        Ok((private_key, public_key))
    }
    
    /// Perform ECDH key agreement and derive shared secret
    pub fn derive_shared_secret(
        private_key: EphemeralPrivateKey,
        peer_public_key: &[u8],
    ) -> Result<Vec<u8>, PairingError> {
        let peer_public = UnparsedPublicKey::new(&X25519, peer_public_key);
        
        ring::agreement::agree_ephemeral(
            private_key,
            &peer_public,
            |shared_secret| shared_secret.to_vec(),
        )
        .map_err(|e| PairingError::CryptoError(format!("ECDH agreement failed: {:?}", e)))
    }
    
    /// Derive encryption key from shared secret using HKDF
    pub fn derive_key(shared_secret: &[u8], info: &[u8]) -> Result<Vec<u8>, PairingError> {
        let salt = hkdf::Salt::new(hkdf::HKDF_SHA256, b"fileshare-pairing-v1");
        let prk = salt.extract(shared_secret);
        
        let mut key = vec![0u8; 32]; // AES-256 key size
        prk.expand(&[info], hkdf::HKDF_SHA256)
            .map_err(|_| PairingError::CryptoError("HKDF expansion failed".to_string()))?
            .fill(&mut key)
            .map_err(|_| PairingError::CryptoError("Failed to fill key buffer".to_string()))?;
        
        Ok(key)
    }
    
    /// Encrypt data using AES-256-GCM
    pub fn encrypt_aes_gcm(
        key: &[u8],
        plaintext: &[u8],
        additional_data: &[u8],
    ) -> Result<(Vec<u8>, Vec<u8>), PairingError> {
        if key.len() != 32 {
            return Err(PairingError::CryptoError("Invalid key length".to_string()));
        }
        
        let unbound_key = UnboundKey::new(&AES_256_GCM, key)
            .map_err(|e| PairingError::CryptoError(format!("Failed to create AES key: {:?}", e)))?;
        let key = LessSafeKey::new(unbound_key);
        
        // Generate random nonce
        let mut nonce_bytes = [0u8; 12];
        let rng = SystemRandom::new();
        rng.fill(&mut nonce_bytes)
            .map_err(|e| PairingError::CryptoError(format!("Failed to generate nonce: {:?}", e)))?;
        let nonce = Nonce::assume_unique_for_key(nonce_bytes);
        
        // Encrypt in place
        let mut ciphertext = plaintext.to_vec();
        key.seal_in_place_append_tag(
            nonce,
            Aad::from(additional_data),
            &mut ciphertext,
        )
        .map_err(|e| PairingError::CryptoError(format!("Encryption failed: {:?}", e)))?;
        
        Ok((ciphertext, nonce_bytes.to_vec()))
    }
    
    /// Decrypt data using AES-256-GCM
    pub fn decrypt_aes_gcm(
        key: &[u8],
        ciphertext: &[u8],
        nonce: &[u8],
        additional_data: &[u8],
    ) -> Result<Vec<u8>, PairingError> {
        if key.len() != 32 {
            return Err(PairingError::CryptoError("Invalid key length".to_string()));
        }
        
        if nonce.len() != 12 {
            return Err(PairingError::CryptoError("Invalid nonce length".to_string()));
        }
        
        let unbound_key = UnboundKey::new(&AES_256_GCM, key)
            .map_err(|e| PairingError::CryptoError(format!("Failed to create AES key: {:?}", e)))?;
        let key = LessSafeKey::new(unbound_key);
        
        let mut nonce_bytes = [0u8; 12];
        nonce_bytes.copy_from_slice(nonce);
        let nonce = Nonce::assume_unique_for_key(nonce_bytes);
        
        // Decrypt in place
        let mut plaintext = ciphertext.to_vec();
        key.open_in_place(
            nonce,
            Aad::from(additional_data),
            &mut plaintext,
        )
        .map_err(|e| PairingError::CryptoError(format!("Decryption failed: {:?}", e)))?;
        
        Ok(plaintext)
    }
    
    /// Sign data using Ed25519
    pub fn sign_ed25519(private_key: &[u8], data: &[u8]) -> Result<Vec<u8>, PairingError> {
        let key_pair = Ed25519KeyPair::from_pkcs8(private_key)
            .map_err(|e| PairingError::CryptoError(format!("Failed to parse Ed25519 key: {:?}", e)))?;
        
        Ok(key_pair.sign(data).as_ref().to_vec())
    }
    
    /// Verify Ed25519 signature
    pub fn verify_ed25519(public_key: &[u8], data: &[u8], signature: &[u8]) -> bool {
        let peer_public_key = ring::signature::UnparsedPublicKey::new(
            &ring::signature::ED25519,
            public_key,
        );
        
        peer_public_key.verify(data, signature).is_ok()
    }
    
    /// Hash data using SHA-256
    pub fn sha256(data: &[u8]) -> Vec<u8> {
        let hash = digest::digest(&digest::SHA256, data);
        hash.as_ref().to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_pin_generation() {
        let pin = PairingCrypto::generate_pin();
        assert_eq!(pin.len(), 6);
        assert!(pin.chars().all(|c| c.is_ascii_digit()));
        
        let pin_num: u32 = pin.parse().unwrap();
        assert!(pin_num >= 100_000 && pin_num < 1_000_000);
    }
    
    #[test]
    fn test_pin_verification() {
        let pin = "123456";
        assert!(PairingCrypto::verify_pin(pin, pin));
        assert!(!PairingCrypto::verify_pin(pin, "654321"));
    }
    
    #[test]
    fn test_ephemeral_keypair() {
        let result = PairingCrypto::generate_ephemeral_keypair();
        assert!(result.is_ok());
        let (_, public_key) = result.unwrap();
        assert_eq!(public_key.len(), 32); // X25519 public key is 32 bytes
    }
    
    #[test]
    fn test_aes_gcm_roundtrip() {
        let key = vec![0u8; 32];
        let plaintext = b"Hello, World!";
        let aad = b"additional data";
        
        let (ciphertext, nonce) = PairingCrypto::encrypt_aes_gcm(&key, plaintext, aad).unwrap();
        let decrypted = PairingCrypto::decrypt_aes_gcm(&key, &ciphertext, &nonce, aad).unwrap();
        
        assert_eq!(plaintext.to_vec(), decrypted);
    }
}