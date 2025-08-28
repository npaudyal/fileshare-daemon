use crate::network::protocol::{Message, MessageType};
use crate::quic::connection::QuicConnectionManager;
use crate::quic::stream_manager::StreamManager;
use crate::{FileshareError, Result};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot, RwLock};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

// Fast pairing timeouts (Bluetooth-style)
const PAIRING_TIMEOUT_SECONDS: u64 = 5; // Much faster than 30s
const CONNECTION_TIMEOUT_SECONDS: u64 = 3;

/// Lightweight pairing request tracker
#[derive(Debug)]
struct PendingPairing {
    target_device_id: Uuid,
    target_address: SocketAddr,
    initiated_at: Instant,
    response_channel: oneshot::Sender<PairingResult>,
}

/// Fast pairing result
#[derive(Debug, Clone)]
pub struct PairingResult {
    pub success: bool,
    pub remote_device_id: Option<Uuid>,
    pub remote_device_name: Option<String>,
    pub error_message: Option<String>,
}

/// Event sent when pairing completes (success or failure)
#[derive(Debug, Clone)]
pub struct PairingCompletedEvent {
    pub pairing_id: Uuid,
    pub result: PairingResult,
}

/// Callback type for pairing completion
pub type PairingCompletedCallback = Arc<dyn Fn(PairingCompletedEvent) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send + Sync>;

/// Lightning-fast pairing manager (Bluetooth-style)
pub struct FastPairingManager {
    device_id: Uuid,
    device_name: String,
    // Dedicated QUIC manager for pairing only
    quic_manager: Arc<QuicConnectionManager>,
    // Minimal state - no complex locks
    pending_pairings: Arc<RwLock<HashMap<Uuid, PendingPairing>>>,
    // Direct message channel for pairing only
    message_tx: mpsc::UnboundedSender<(Uuid, Message)>,
    message_rx: Option<mpsc::UnboundedReceiver<(Uuid, Message)>>,
    // Active connections for pairing
    active_connections: Arc<RwLock<HashMap<Uuid, Arc<StreamManager>>>>,
    // Event callback for completion
    completion_callback: Option<PairingCompletedCallback>,
}

impl FastPairingManager {
    pub fn new(
        device_id: Uuid,
        device_name: String,
        quic_manager: Arc<QuicConnectionManager>,
    ) -> Self {
        info!("🚀 Initializing FastPairingManager (Bluetooth-style)");
        
        // Create dedicated message channel for pairing
        let (message_tx, message_rx) = mpsc::unbounded_channel();
        
        Self {
            device_id,
            device_name,
            quic_manager,
            pending_pairings: Arc::new(RwLock::new(HashMap::new())),
            message_tx,
            message_rx: Some(message_rx),
            active_connections: Arc::new(RwLock::new(HashMap::new())),
            completion_callback: None,
        }
    }

    /// Start the fast pairing message processor
    pub async fn start_message_processor(&mut self) -> Result<()> {
        let message_rx = self.message_rx.take().ok_or_else(|| {
            FileshareError::Unknown("Message processor already started".to_string())
        })?;

        let pending_pairings = self.pending_pairings.clone();
        let active_connections = self.active_connections.clone();
        let completion_callback = self.completion_callback.clone();

        // Spawn lightweight message processor (Bluetooth-style)
        tokio::spawn(async move {
            Self::process_fast_pairing_messages(
                message_rx,
                pending_pairings,
                active_connections,
                completion_callback,
            ).await;
        });

        info!("⚡ FastPairingManager message processor started");
        Ok(())
    }

    /// Process pairing messages (lockless, fast)
    async fn process_fast_pairing_messages(
        mut message_rx: mpsc::UnboundedReceiver<(Uuid, Message)>,
        pending_pairings: Arc<RwLock<HashMap<Uuid, PendingPairing>>>,
        _active_connections: Arc<RwLock<HashMap<Uuid, Arc<StreamManager>>>>,
        completion_callback: Option<PairingCompletedCallback>,
    ) {
        info!("⚡ Fast pairing message processor started");
        
        while let Some((peer_id, message)) = message_rx.recv().await {
            let start_time = Instant::now();
            info!("📨 Fast pairing received: {:?} from {}", message.message_type, peer_id);
            
            match message.message_type {
                MessageType::PairingResult { success, device_id, device_name, reason } => {
                    Self::handle_pairing_result(
                        peer_id,
                        success,
                        device_id,
                        device_name,
                        reason,
                        &pending_pairings,
                        &completion_callback,
                    ).await;
                }
                MessageType::HandshakeResponse { accepted, reason: _ } => {
                    info!("🤝 Fast pairing handshake response: accepted={}", accepted);
                    // For fast pairing, we don't need to do much with handshake responses
                }
                _ => {
                    debug!("📨 Ignoring non-pairing message in fast pairing: {:?}", message.message_type);
                }
            }
            
            let processing_time = start_time.elapsed();
            if processing_time.as_millis() > 10 {
                warn!("⏱️ Fast pairing message took {:?}", processing_time);
            } else {
                debug!("⚡ Fast pairing message processed in {:?}", processing_time);
            }
        }
        
        info!("⚡ Fast pairing message processor stopped");
    }

    pub fn set_completion_callback(&mut self, callback: PairingCompletedCallback) {
        self.completion_callback = Some(callback);
    }

    /// Handle pairing result (fast, lockless)
    async fn handle_pairing_result(
        peer_id: Uuid,
        success: bool,
        remote_device_id: Option<Uuid>,
        remote_device_name: Option<String>,
        reason: Option<String>,
        pending_pairings: &Arc<RwLock<HashMap<Uuid, PendingPairing>>>,
        _completion_callback: &Option<PairingCompletedCallback>,
    ) {
        info!("⚡ Fast pairing result: success={}, peer={}", success, peer_id);
        
        // Find and remove the pending pairing
        let mut pairing_to_complete = None;
        {
            let mut pending = pending_pairings.write().await;
            // Find by target device ID
            if let Some(target_id) = remote_device_id {
                let mut pairing_id_to_remove = None;
                for (pairing_id, pending_pairing) in pending.iter() {
                    if pending_pairing.target_device_id == target_id {
                        pairing_id_to_remove = Some(*pairing_id);
                        break;
                    }
                }
                if let Some(pairing_id) = pairing_id_to_remove {
                    pairing_to_complete = pending.remove(&pairing_id);
                }
            }
            
            // Fallback: if only one pending pairing, assume it's this one
            if pairing_to_complete.is_none() && pending.len() == 1 {
                let (pairing_id, _pending_pairing) = pending.iter().next().unwrap();
                let pairing_id = *pairing_id;
                pairing_to_complete = pending.remove(&pairing_id);
                info!("⚡ Using only pending pairing as fallback: {}", pairing_id);
            }
        }
        
        if let Some(pending_pairing) = pairing_to_complete {
            let result = PairingResult {
                success,
                remote_device_id,
                remote_device_name,
                error_message: if success { None } else { reason },
            };
            
            // Send result to waiting channel (non-blocking)
            if let Err(_) = pending_pairing.response_channel.send(result.clone()) {
                warn!("⚠️ Fast pairing channel closed for peer {}", peer_id);
            } else {
                info!("✅ Fast pairing result sent to waiting channel: {}", success);
            }
        } else {
            warn!("⚠️ No pending pairing found for peer {}", peer_id);
        }
    }

    /// Initiate fast pairing (Bluetooth-style)
    pub async fn initiate_pairing(
        &self,
        target_device_id: Uuid,
        target_address: SocketAddr,
        pin: String,
    ) -> Result<PairingResult> {
        let pairing_id = Uuid::new_v4();
        info!("⚡ Initiating FAST pairing {} with device {}", pairing_id, target_device_id);
        
        // Create response channel
        let (tx, rx) = oneshot::channel();
        
        // Create pending pairing (minimal state)
        let pending = PendingPairing {
            target_device_id,
            target_address,
            initiated_at: Instant::now(),
            response_channel: tx,
        };
        
        // Store pending pairing
        {
            let mut pending_map = self.pending_pairings.write().await;
            pending_map.insert(pairing_id, pending);
            info!("📝 Stored fast pairing request {}", pairing_id);
        }
        
        // Start fast pairing process
        let _result = self.execute_fast_pairing(pairing_id, target_device_id, target_address, pin).await;
        
        // Wait for result with fast timeout
        match tokio::time::timeout(Duration::from_secs(PAIRING_TIMEOUT_SECONDS), rx).await {
            Ok(Ok(pairing_result)) => {
                info!("⚡ Fast pairing {} completed: {}", pairing_id, pairing_result.success);
                
                // Notify completion callback
                if let Some(callback) = &self.completion_callback {
                    let event = PairingCompletedEvent {
                        pairing_id,
                        result: pairing_result.clone(),
                    };
                    tokio::spawn({
                        let callback = callback.clone();
                        async move { callback(event).await }
                    });
                }
                
                Ok(pairing_result)
            }
            Ok(Err(_)) => {
                error!("❌ Fast pairing {} channel closed", pairing_id);
                self.cleanup_pairing(pairing_id).await;
                Err(FileshareError::Unknown("Pairing channel closed".to_string()))
            }
            Err(_) => {
                error!("⏰ Fast pairing {} timed out ({}s)", pairing_id, PAIRING_TIMEOUT_SECONDS);
                self.cleanup_pairing(pairing_id).await;
                Err(FileshareError::Unknown(format!("Pairing timed out after {}s", PAIRING_TIMEOUT_SECONDS)))
            }
        }
    }

    /// Execute the fast pairing protocol (Bluetooth-style)
    async fn execute_fast_pairing(
        &self,
        pairing_id: Uuid,
        target_device_id: Uuid,
        target_address: SocketAddr,
        pin: String,
    ) -> Result<()> {
        info!("🔗 Fast connecting to {} for pairing {}", target_address, pairing_id);
        
        // Direct connection (no discovery dependency)
        let connection = tokio::time::timeout(
            Duration::from_secs(CONNECTION_TIMEOUT_SECONDS),
            self.quic_manager.connect_to_peer(target_address, target_device_id)
        ).await.map_err(|_| {
            FileshareError::Network(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("Connection timeout ({}s)", CONNECTION_TIMEOUT_SECONDS)
            ))
        })??;

        info!("✅ Fast connection established to {}", target_address);
        
        // Create StreamManager for this connection with our message channel
        let stream_manager = Arc::new(StreamManager::new(
            connection,
            self.message_tx.clone(),
        ));
        
        // Store the active connection
        {
            let mut active_conns = self.active_connections.write().await;
            active_conns.insert(target_device_id, stream_manager.clone());
        }
        
        // Send handshake first (required by protocol)
        let handshake = Message::new(MessageType::Handshake {
            device_id: self.device_id,
            device_name: self.device_name.clone(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        });
        
        info!("🤝 Sending fast handshake for pairing {}", pairing_id);
        stream_manager.send_control_message(handshake).await.map_err(|e| {
            error!("❌ Failed to send handshake: {}", e);
            e
        })?;
        
        // Small delay for handshake to complete
        tokio::time::sleep(Duration::from_millis(100)).await;
        
        // Hash PIN
        let pin_hash = self.hash_pin(&pin);
        
        // Create pairing request (simplified)
        let pairing_request = Message::new(MessageType::PairingRequest {
            device_id: self.device_id,
            device_name: self.device_name.clone(),
            pin_hash,
            platform: Some(self.get_platform()),
        });
        
        info!("📤 Sending fast pairing request for {}", pairing_id);
        
        // Send pairing request directly
        stream_manager.send_control_message(pairing_request).await.map_err(|e| {
            error!("❌ Failed to send pairing request: {}", e);
            // Clean up connection on failure
            let active_conns = self.active_connections.clone();
            tokio::spawn(async move {
                let mut conns = active_conns.write().await;
                conns.remove(&target_device_id);
            });
            e
        })?;
        
        info!("⚡ Fast pairing request sent successfully for {}", pairing_id);
        Ok(())
    }

    /// Clean up pairing state
    async fn cleanup_pairing(&self, pairing_id: Uuid) {
        let mut pending_map = self.pending_pairings.write().await;
        if pending_map.remove(&pairing_id).is_some() {
            info!("🧹 Cleaned up fast pairing {}", pairing_id);
        }
    }

    /// Hash PIN (same as current implementation)
    fn hash_pin(&self, pin: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(pin.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Get platform string
    fn get_platform(&self) -> String {
        if cfg!(target_os = "macos") {
            "macOS".to_string()
        } else if cfg!(target_os = "windows") {
            "Windows".to_string()
        } else if cfg!(target_os = "linux") {
            "Linux".to_string()
        } else {
            "Unknown".to_string()
        }
    }

    /// Get stats for debugging
    pub async fn get_stats(&self) -> (usize, Vec<Uuid>) {
        let pending = self.pending_pairings.read().await;
        let count = pending.len();
        let ids = pending.keys().copied().collect();
        (count, ids)
    }
}