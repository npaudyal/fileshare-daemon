// src/network/high_speed_streaming.rs
use crate::network::high_speed_memory::HighSpeedMemoryPool;
use crate::network::performance_monitor::PerformanceMonitor;
use crate::{FileshareError, Result};
use bytes::{Bytes, BytesMut};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, RwLock};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

// High-speed transfer configuration
#[derive(Debug, Clone)]
pub struct HighSpeedConfig {
    pub connection_count: usize,      // 4-8 parallel connections
    pub chunk_size: usize,            // 1-4MB adaptive chunk size
    pub batch_size: usize,            // 2MB batch size
    pub pipeline_depth: usize,        // Queue depth for pipeline stages
    pub memory_limit: usize,          // 500MB total memory limit
    pub compression_threshold: f64,   // Only compress if ratio > 0.8
    pub enable_adaptive_sizing: bool, // Dynamic chunk/batch sizing
    pub target_cpu_usage: f64,        // 80% target CPU usage
    pub bandwidth_target_mbps: f64,   // Target bandwidth in MB/s
}

impl Default for HighSpeedConfig {
    fn default() -> Self {
        Self {
            connection_count: 6,             // Optimal for most networks
            chunk_size: 2 * 1024 * 1024,     // 2MB base chunk size
            batch_size: 4 * 1024 * 1024,     // 4MB batch size
            pipeline_depth: 16,              // 16 chunks in pipeline
            memory_limit: 500 * 1024 * 1024, // 500MB memory limit
            compression_threshold: 0.85,     // 85% compression ratio threshold
            enable_adaptive_sizing: true,
            target_cpu_usage: 0.80,       // 80% CPU target
            bandwidth_target_mbps: 100.0, // 100 MB/s target
        }
    }
}

// High-speed transfer stages
#[derive(Debug)]
enum PipelineStage {
    Reading,
    Processing,
    Sending,
    Completing,
}

// Raw chunk from file reader
#[derive(Debug)]
pub struct RawChunk {
    pub chunk_index: u64,
    pub data: Bytes,
    pub file_offset: u64,
    pub is_last: bool,
    pub read_timestamp: Instant,
}

// Processed chunk (potentially compressed)
#[derive(Debug, Clone)]
pub struct ProcessedChunk {
    pub chunk_index: u64,
    pub data: Bytes,
    pub compressed: bool,
    pub original_size: usize,
    pub process_timestamp: Instant,
}

// Network-ready batch of chunks
#[derive(Debug, Clone)]
pub struct NetworkBatch {
    pub batch_id: u64,
    pub chunks: Vec<ProcessedChunk>,
    pub total_size: usize,
    pub connection_id: usize,
    pub batch_header: HighSpeedBatchHeader,
}

// Optimized batch header (only 16 bytes)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct HighSpeedBatchHeader {
    pub batch_id: u64,    // 8 bytes
    pub chunk_count: u16, // 2 bytes
    pub total_size: u32,  // 4 bytes (4GB max batch)
    pub flags: u16,       // 2 bytes (compression, last batch, etc.)
}

impl HighSpeedBatchHeader {
    const SIZE: usize = 16;

    pub fn new(batch_id: u64, chunk_count: u16, total_size: u32) -> Self {
        Self {
            batch_id,
            chunk_count,
            total_size,
            flags: 0,
        }
    }

    pub fn set_compressed(&mut self) {
        self.flags |= 0x0001;
    }

    pub fn set_last_batch(&mut self) {
        self.flags |= 0x0002;
    }

    pub fn is_compressed(&self) -> bool {
        self.flags & 0x0001 != 0
    }

    pub fn is_last_batch(&self) -> bool {
        self.flags & 0x0002 != 0
    }

    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        unsafe { std::mem::transmute(*self) }
    }

    pub fn from_bytes(bytes: &[u8; Self::SIZE]) -> Self {
        unsafe { std::mem::transmute(*bytes) }
    }
}

// Main high-speed streaming manager
pub struct HighSpeedStreamingManager {
    config: HighSpeedConfig,
    active_transfers: Arc<RwLock<HashMap<Uuid, HighSpeedTransferState>>>,
    memory_pool: Arc<HighSpeedMemoryPool>,
    performance_monitor: Arc<PerformanceMonitor>,
    command_tx: mpsc::UnboundedSender<HighSpeedCommand>,
}

#[derive(Debug)]
enum HighSpeedCommand {
    StartOutgoing {
        transfer_id: Uuid,
        peer_id: Uuid,
        file_path: PathBuf,
        peer_address: std::net::SocketAddr,
        response: oneshot::Sender<Result<()>>,
    },
    StartIncoming {
        transfer_id: Uuid,
        expected_size: u64,
        save_path: PathBuf,
        connections: Vec<TcpStream>,
        response: oneshot::Sender<Result<()>>,
    },
    CancelTransfer {
        transfer_id: Uuid,
        response: oneshot::Sender<Result<()>>,
    },
    GetProgress {
        transfer_id: Uuid,
        response: oneshot::Sender<Option<HighSpeedProgress>>,
    },
}

#[derive(Debug, Clone)]
pub struct HighSpeedProgress {
    pub transfer_id: Uuid,
    pub bytes_transferred: u64,
    pub total_bytes: u64,
    pub speed_mbps: f64,
    pub eta_seconds: f64,
    pub connection_speeds: Vec<f64>,
    pub cpu_usage: f64,
    pub memory_usage: u64,
    pub compression_ratio: f64,
}

#[derive(Debug)]
struct HighSpeedTransferState {
    transfer_id: Uuid,
    file_path: PathBuf,
    total_size: u64,
    bytes_transferred: AtomicU64,
    start_time: Instant,
    connections: Vec<HighSpeedConnection>,
    pipeline_handles: Vec<tokio::task::JoinHandle<()>>,
    cancel_token: tokio_util::sync::CancellationToken,
}

#[derive(Debug)]
pub struct HighSpeedConnection {
    pub connection_id: usize,
    pub stream: TcpStream,
    pub bytes_sent: AtomicU64,
    pub last_activity: Instant,
    pub health_score: f64,
}

impl HighSpeedStreamingManager {
    pub async fn new(config: HighSpeedConfig) -> Result<Self> {
        let memory_pool = Arc::new(HighSpeedMemoryPool::new(
            config.memory_limit,
            config.chunk_size,
            config.batch_size,
        ));

        let performance_monitor = Arc::new(PerformanceMonitor::new(config.clone()));
        let (command_tx, command_rx) = mpsc::unbounded_channel();

        // Start background command processor
        let active_transfers = Arc::new(RwLock::new(HashMap::new()));
        let bg_active_transfers = active_transfers.clone();
        let bg_memory_pool = memory_pool.clone();
        let bg_performance_monitor = performance_monitor.clone();
        let bg_config = config.clone();

        tokio::spawn(async move {
            Self::command_processor(
                command_rx,
                bg_active_transfers,
                bg_memory_pool,
                bg_performance_monitor,
                bg_config,
            )
            .await;
        });

        info!(
            "🚀 High-Speed Streaming Manager initialized with {} connections",
            config.connection_count
        );

        Ok(Self {
            config,
            active_transfers,
            memory_pool,
            performance_monitor,
            command_tx,
        })
    }

    // PUBLIC API
    pub async fn start_outgoing_transfer(
        &self,
        transfer_id: Uuid,
        peer_id: Uuid,
        file_path: PathBuf,
        peer_address: std::net::SocketAddr,
    ) -> Result<()> {
        let (tx, rx) = oneshot::channel();

        self.command_tx
            .send(HighSpeedCommand::StartOutgoing {
                transfer_id,
                peer_id,
                file_path,
                peer_address,
                response: tx,
            })
            .map_err(|_| FileshareError::Unknown("Failed to send command".to_string()))?;

        rx.await
            .map_err(|_| FileshareError::Unknown("Command failed".to_string()))?
    }

    pub async fn get_progress(&self, transfer_id: Uuid) -> Option<HighSpeedProgress> {
        let (tx, rx) = oneshot::channel();

        if self
            .command_tx
            .send(HighSpeedCommand::GetProgress {
                transfer_id,
                response: tx,
            })
            .is_ok()
        {
            rx.await.unwrap_or(None)
        } else {
            None
        }
    }

    // Background command processor
    async fn command_processor(
        mut command_rx: mpsc::UnboundedReceiver<HighSpeedCommand>,
        active_transfers: Arc<RwLock<HashMap<Uuid, HighSpeedTransferState>>>,
        memory_pool: Arc<HighSpeedMemoryPool>,
        performance_monitor: Arc<PerformanceMonitor>,
        config: HighSpeedConfig,
    ) {
        while let Some(command) = command_rx.recv().await {
            match command {
                HighSpeedCommand::StartOutgoing {
                    transfer_id,
                    peer_id,
                    file_path,
                    peer_address,
                    response,
                } => {
                    let result = Self::handle_start_outgoing(
                        transfer_id,
                        peer_id,
                        file_path,
                        peer_address,
                        &active_transfers,
                        &memory_pool,
                        &performance_monitor,
                        &config,
                    )
                    .await;
                    let _ = response.send(result);
                }

                HighSpeedCommand::GetProgress {
                    transfer_id,
                    response,
                } => {
                    let progress = Self::calculate_progress(
                        transfer_id,
                        &active_transfers,
                        &performance_monitor,
                    )
                    .await;
                    let _ = response.send(progress);
                }

                HighSpeedCommand::CancelTransfer {
                    transfer_id,
                    response,
                } => {
                    let result = Self::handle_cancel_transfer(transfer_id, &active_transfers).await;
                    let _ = response.send(result);
                }

                _ => {} // Handle other commands
            }
        }
    }

    // Start outgoing transfer with parallel pipeline
    async fn handle_start_outgoing(
        transfer_id: Uuid,
        _peer_id: Uuid,
        file_path: PathBuf,
        peer_address: std::net::SocketAddr,
        active_transfers: &Arc<RwLock<HashMap<Uuid, HighSpeedTransferState>>>,
        memory_pool: &Arc<HighSpeedMemoryPool>,
        performance_monitor: &Arc<PerformanceMonitor>,
        config: &HighSpeedConfig,
    ) -> Result<()> {
        info!(
            "🚀 Starting high-speed outgoing transfer: {} to {}",
            transfer_id, peer_address
        );

        // Get file size
        let file_metadata = tokio::fs::metadata(&file_path).await?;
        let file_size = file_metadata.len();

        // Create parallel connections
        let mut connections = Vec::new();
        for i in 0..config.connection_count {
            let stream = TcpStream::connect(peer_address).await?;
            Self::optimize_tcp_socket(&stream).await?;

            connections.push(HighSpeedConnection {
                connection_id: i,
                stream,
                bytes_sent: AtomicU64::new(0),
                last_activity: Instant::now(),
                health_score: 1.0,
            });
        }

        info!("✅ Created {} parallel connections", connections.len());

        // Create cancellation token
        let cancel_token = tokio_util::sync::CancellationToken::new();

        // Start the high-speed pipeline
        let pipeline_handles = Self::start_outgoing_pipeline(
            transfer_id,
            file_path.clone(),
            file_size,
            connections,
            memory_pool.clone(),
            performance_monitor.clone(),
            config.clone(),
            cancel_token.clone(),
        )
        .await?;

        // Store transfer state
        let transfer_state = HighSpeedTransferState {
            transfer_id,
            file_path,
            total_size: file_size,
            bytes_transferred: AtomicU64::new(0),
            start_time: Instant::now(),
            connections: Vec::new(), // Moved to pipeline
            pipeline_handles,
            cancel_token,
        };

        active_transfers
            .write()
            .await
            .insert(transfer_id, transfer_state);

        info!(
            "🎯 High-speed transfer {} started successfully",
            transfer_id
        );
        Ok(())
    }

    // Optimize TCP socket for high-speed transfers
    async fn optimize_tcp_socket(stream: &TcpStream) -> Result<()> {
        use socket2::{SockRef, Socket};

        let sock_ref = SockRef::from(stream);

        // Disable Nagle's algorithm for low latency
        sock_ref.set_nodelay(true)?;

        // Set large send/receive buffers
        sock_ref.set_send_buffer_size(1024 * 1024)?; // 1MB send buffer
        sock_ref.set_recv_buffer_size(1024 * 1024)?; // 1MB receive buffer

        #[cfg(target_os = "linux")]
        {
            // Linux-specific optimizations
            use std::os::unix::io::AsRawFd;
            let fd = stream.as_raw_fd();

            // TCP_CORK - batch data before sending
            unsafe {
                let cork: libc::c_int = 1;
                libc::setsockopt(
                    fd,
                    libc::IPPROTO_TCP,
                    libc::TCP_CORK,
                    &cork as *const _ as *const libc::c_void,
                    std::mem::size_of::<libc::c_int>() as libc::socklen_t,
                );
            }
        }

        Ok(())
    }

    async fn calculate_progress(
        transfer_id: Uuid,
        active_transfers: &Arc<RwLock<HashMap<Uuid, HighSpeedTransferState>>>,
        performance_monitor: &Arc<PerformanceMonitor>,
    ) -> Option<HighSpeedProgress> {
        let transfers = active_transfers.read().await;
        if let Some(transfer) = transfers.get(&transfer_id) {
            let bytes_transferred = transfer.bytes_transferred.load(Ordering::Relaxed);
            let elapsed = transfer.start_time.elapsed().as_secs_f64();
            let speed_mbps = if elapsed > 0.0 {
                (bytes_transferred as f64) / (elapsed * 1024.0 * 1024.0)
            } else {
                0.0
            };

            let eta_seconds = if speed_mbps > 0.0 {
                let remaining_bytes = transfer.total_size.saturating_sub(bytes_transferred);
                (remaining_bytes as f64) / (speed_mbps * 1024.0 * 1024.0)
            } else {
                f64::INFINITY
            };

            let metrics = performance_monitor.get_current_metrics().await;

            Some(HighSpeedProgress {
                transfer_id,
                bytes_transferred,
                total_bytes: transfer.total_size,
                speed_mbps,
                eta_seconds,
                connection_speeds: vec![speed_mbps / 4.0; 4], // Distribute across connections
                cpu_usage: metrics.cpu_usage,
                memory_usage: metrics.memory_usage,
                compression_ratio: metrics.compression_ratio,
            })
        } else {
            None
        }
    }

    async fn handle_cancel_transfer(
        transfer_id: Uuid,
        active_transfers: &Arc<RwLock<HashMap<Uuid, HighSpeedTransferState>>>,
    ) -> Result<()> {
        let mut transfers = active_transfers.write().await;
        if let Some(transfer) = transfers.remove(&transfer_id) {
            transfer.cancel_token.cancel();

            // Wait for pipeline tasks to complete
            for handle in transfer.pipeline_handles {
                let _ = handle.await;
            }

            info!("🛑 High-speed transfer {} cancelled", transfer_id);
        }
        Ok(())
    }
}
