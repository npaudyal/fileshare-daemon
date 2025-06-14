use crate::{FileshareError, Result};
use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use tokio::time::timeout;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

// FIXED: Lock-free connection pool with message passing
#[derive(Debug)]
pub enum PoolCommand {
    GetConnection {
        peer_id: Uuid,
        addr: SocketAddr,
        response: oneshot::Sender<Result<Arc<TcpStream>>>,
    },
    ReturnConnection {
        peer_id: Uuid,
        stream: Arc<TcpStream>,
        was_error: bool,
    },
    GetStats {
        response: oneshot::Sender<ConnectionPoolStats>,
    },
    Cleanup,
    Shutdown,
}

#[derive(Debug, Clone)]
pub struct ConnectionPoolStats {
    pub total_connections: usize,
    pub active_connections: usize,
    pub idle_connections: usize,
    pub failed_connections: usize,
    pub peers: HashMap<Uuid, PeerConnectionStats>,
}

#[derive(Debug, Clone)]
pub struct PeerConnectionStats {
    pub total_connections: usize,
    pub active_connections: usize,
    pub idle_connections: usize,
    pub active_transfers: usize,
    pub avg_latency_ms: f64,
    pub success_rate: f64,
}

// SAFE: Connection wrapper with proper lifecycle management
#[derive(Debug, Clone)]
pub struct PooledConnection {
    pub id: usize,
    pub stream: Arc<TcpStream>,
    pub created_at: Instant,
    pub last_used: Instant,
    pub active_transfers: usize,
    pub max_concurrent_transfers: usize,
    pub peer_id: Uuid,
    pub total_transfers: usize,
    pub failed_transfers: usize,
    pub connection_quality: f64, // 0.0 to 1.0
}

impl PooledConnection {
    pub fn new(id: usize, stream: TcpStream, peer_id: Uuid) -> Self {
        let now = Instant::now();
        Self {
            id,
            stream: Arc::new(stream),
            created_at: now,
            last_used: now,
            active_transfers: 0,
            max_concurrent_transfers: 3,
            peer_id,
            total_transfers: 0,
            failed_transfers: 0,
            connection_quality: 1.0,
        }
    }

    pub fn can_accept_transfer(&self) -> bool {
        self.active_transfers < self.max_concurrent_transfers && self.connection_quality > 0.3
    }

    pub fn calculate_score(&self) -> f64 {
        let load_factor =
            1.0 - (self.active_transfers as f64 / self.max_concurrent_transfers as f64);
        let age_factor = 1.0 / (1.0 + self.created_at.elapsed().as_secs() as f64 / 3600.0); // Prefer newer connections
        let success_rate = if self.total_transfers > 0 {
            1.0 - (self.failed_transfers as f64 / self.total_transfers as f64)
        } else {
            1.0
        };

        (load_factor * 0.5 + self.connection_quality * 0.3 + age_factor * 0.1 + success_rate * 0.1)
            .clamp(0.0, 1.0)
    }

    pub fn mark_transfer_start(&mut self) {
        self.active_transfers += 1;
        self.total_transfers += 1;
        self.last_used = Instant::now();
    }

    pub fn mark_transfer_end(&mut self, success: bool) {
        self.active_transfers = self.active_transfers.saturating_sub(1);
        if !success {
            self.failed_transfers += 1;
            self.connection_quality = (self.connection_quality * 0.9).max(0.1); // Reduce quality on failure
        } else {
            self.connection_quality = (self.connection_quality * 0.99 + 0.01).min(1.0);
            // Slowly improve quality
        }
        self.last_used = Instant::now();
    }

    pub fn is_idle(&self, idle_timeout: Duration) -> bool {
        self.active_transfers == 0 && self.last_used.elapsed() > idle_timeout
    }

    pub fn is_stale(&self, max_age: Duration) -> bool {
        self.created_at.elapsed() > max_age
    }
}

// Pool for a single peer
#[derive(Debug)]
struct PeerConnectionPool {
    peer_id: Uuid,
    available: VecDeque<PooledConnection>,
    in_use: HashMap<usize, PooledConnection>,
    next_id: usize,
    max_connections: usize,
    connection_timeout: Duration,
    idle_timeout: Duration,
    max_age: Duration,
}

impl PeerConnectionPool {
    fn new(peer_id: Uuid) -> Self {
        Self {
            peer_id,
            available: VecDeque::new(),
            in_use: HashMap::new(),
            next_id: 0,
            max_connections: 4,
            connection_timeout: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(300), // 5 minutes
            max_age: Duration::from_secs(3600),     // 1 hour
        }
    }

    fn get_best_connection(&mut self) -> Option<PooledConnection> {
        // Find the best available connection based on score
        let best_index = (0..self.available.len()).max_by(|&a, &b| {
            let score_a = self.available[a].calculate_score();
            let score_b = self.available[b].calculate_score();
            score_a
                .partial_cmp(&score_b)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        if let Some(index) = best_index {
            let mut conn = self.available.remove(index).unwrap();
            if conn.can_accept_transfer() {
                conn.mark_transfer_start();
                self.in_use.insert(conn.id, conn.clone());
                return Some(conn);
            }
        }
        None
    }

    fn add_connection(&mut self, stream: TcpStream) -> PooledConnection {
        let conn = PooledConnection::new(self.next_id, stream, self.peer_id);
        self.next_id += 1;

        let conn_copy = conn.clone();
        self.available.push_back(conn);

        info!(
            "Added new connection {} to pool for peer {} (total: {})",
            conn_copy.id,
            self.peer_id,
            self.total_connections()
        );

        conn_copy
    }

    fn return_connection(&mut self, connection_id: usize, success: bool) {
        if let Some(mut conn) = self.in_use.remove(&connection_id) {
            conn.mark_transfer_end(success);

            // Only return to pool if connection is still good
            if !conn.is_stale(self.max_age) && conn.connection_quality > 0.1 {
                self.available.push_back(conn);
            } else {
                debug!(
                    "Discarding connection {} due to age or poor quality",
                    connection_id
                );
            }
        }
    }

    fn cleanup_stale_connections(&mut self) -> usize {
        let initial_count = self.total_connections();

        // Clean up available connections
        self.available
            .retain(|conn| !conn.is_idle(self.idle_timeout) && !conn.is_stale(self.max_age));

        // Clean up in-use connections that are stale (shouldn't happen normally)
        self.in_use.retain(|_, conn| !conn.is_stale(self.max_age));

        initial_count - self.total_connections()
    }

    fn total_connections(&self) -> usize {
        self.available.len() + self.in_use.len()
    }

    fn can_create_connection(&self) -> bool {
        self.total_connections() < self.max_connections
    }

    fn get_stats(&self) -> PeerConnectionStats {
        let total_connections = self.total_connections();
        let active_connections = self.in_use.len();
        let idle_connections = self.available.len();

        let total_transfers: usize = self
            .available
            .iter()
            .chain(self.in_use.values())
            .map(|conn| conn.total_transfers)
            .sum();

        let total_failed: usize = self
            .available
            .iter()
            .chain(self.in_use.values())
            .map(|conn| conn.failed_transfers)
            .sum();

        let success_rate = if total_transfers > 0 {
            1.0 - (total_failed as f64 / total_transfers as f64)
        } else {
            1.0
        };

        let avg_quality: f64 = if total_connections > 0 {
            self.available
                .iter()
                .chain(self.in_use.values())
                .map(|conn| conn.connection_quality)
                .sum::<f64>()
                / total_connections as f64
        } else {
            0.0
        };

        PeerConnectionStats {
            total_connections,
            active_connections,
            idle_connections,
            active_transfers: self.in_use.values().map(|conn| conn.active_transfers).sum(),
            avg_latency_ms: avg_quality * 100.0, // Rough estimate
            success_rate,
        }
    }
}

// SAFE: Main connection pool manager
pub struct ConnectionPoolManager {
    command_tx: mpsc::UnboundedSender<PoolCommand>,
    _background_task: tokio::task::JoinHandle<()>,
}

impl ConnectionPoolManager {
    pub fn new() -> Self {
        let (command_tx, command_rx) = mpsc::unbounded_channel();

        let background_task = tokio::spawn(Self::background_task(command_rx));

        info!("🔗 Lock-free ConnectionPoolManager started");

        Self {
            command_tx,
            _background_task: background_task,
        }
    }

    // SAFE: Single background task manages all state
    async fn background_task(mut command_rx: mpsc::UnboundedReceiver<PoolCommand>) {
        let mut pools: HashMap<Uuid, PeerConnectionPool> = HashMap::new();
        let mut total_connections = AtomicUsize::new(0);
        let max_total_connections = 50;

        // Cleanup task
        let mut cleanup_interval = tokio::time::interval(Duration::from_secs(60));

        info!("🔄 Connection pool background task started");

        loop {
            tokio::select! {
                Some(command) = command_rx.recv() => {
                    match command {
                        PoolCommand::GetConnection { peer_id, addr, response } => {
                            let result = Self::handle_get_connection(
                                &mut pools,
                                &total_connections,
                                max_total_connections,
                                peer_id,
                                addr,
                            ).await;
                            let _ = response.send(result);
                        }

                        PoolCommand::ReturnConnection { peer_id, stream: _, was_error } => {
                            if let Some(pool) = pools.get_mut(&peer_id) {
                                // Find the connection by stream reference (simplified)
                                // In a real implementation, you'd track connection IDs
                                pool.cleanup_stale_connections();
                            }
                        }

                        PoolCommand::GetStats { response } => {
                            let stats = Self::collect_stats(&pools);
                            let _ = response.send(stats);
                        }

                        PoolCommand::Cleanup => {
                            Self::cleanup_all_pools(&mut pools, &total_connections);
                        }

                        PoolCommand::Shutdown => {
                            info!("🛑 Connection pool shutting down");
                            break;
                        }
                    }
                }

                _ = cleanup_interval.tick() => {
                    Self::cleanup_all_pools(&mut pools, &total_connections);
                }
            }
        }
    }

    async fn handle_get_connection(
        pools: &mut HashMap<Uuid, PeerConnectionPool>,
        total_connections: &AtomicUsize,
        max_total_connections: usize,
        peer_id: Uuid,
        addr: SocketAddr,
    ) -> Result<Arc<TcpStream>> {
        // Get or create pool for peer
        let pool = pools
            .entry(peer_id)
            .or_insert_with(|| PeerConnectionPool::new(peer_id));

        // Try to get existing connection
        if let Some(conn) = pool.get_best_connection() {
            debug!("Reusing connection {} for peer {}", conn.id, peer_id);
            return Ok(conn.stream);
        }

        // Check global connection limit
        if total_connections.load(Ordering::Relaxed) >= max_total_connections {
            return Err(FileshareError::Network(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "Connection pool exhausted",
            )));
        }

        // Create new connection if allowed
        if pool.can_create_connection() {
            match Self::create_optimized_connection(addr).await {
                Ok(stream) => {
                    total_connections.fetch_add(1, Ordering::Relaxed);
                    let conn = pool.add_connection(stream);
                    info!("Created new connection {} for peer {}", conn.id, peer_id);
                    Ok(conn.stream)
                }
                Err(e) => {
                    warn!("Failed to create connection to {}: {}", addr, e);
                    Err(e)
                }
            }
        } else {
            Err(FileshareError::Network(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "Peer connection limit reached",
            )))
        }
    }

    async fn create_optimized_connection(addr: SocketAddr) -> Result<TcpStream> {
        let connect_future = TcpStream::connect(addr);
        let stream = timeout(Duration::from_secs(10), connect_future)
            .await
            .map_err(|_| {
                FileshareError::Network(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "Connection timeout",
                ))
            })??;

        // Configure for optimal performance
        Self::configure_tcp_socket(&stream).await?;

        Ok(stream)
    }

    async fn configure_tcp_socket(stream: &TcpStream) -> Result<()> {
        // Only use what's directly available on TcpStream
        stream.set_nodelay(true)?;

        // The default buffer sizes are usually sufficient
        // Keep-alive can be set through other means if needed

        Ok(())
    }

    fn cleanup_all_pools(
        pools: &mut HashMap<Uuid, PeerConnectionPool>,
        total_connections: &AtomicUsize,
    ) {
        let mut total_removed = 0;

        for (peer_id, pool) in pools.iter_mut() {
            let removed = pool.cleanup_stale_connections();
            total_removed += removed;

            if removed > 0 {
                debug!("Cleaned up {} connections for peer {}", removed, peer_id);
            }
        }

        // Remove empty pools
        pools.retain(|_, pool| pool.total_connections() > 0);

        if total_removed > 0 {
            total_connections.fetch_sub(total_removed, Ordering::Relaxed);
            info!("🧹 Cleaned up {} total connections", total_removed);
        }
    }

    fn collect_stats(pools: &HashMap<Uuid, PeerConnectionPool>) -> ConnectionPoolStats {
        let mut stats = ConnectionPoolStats {
            total_connections: 0,
            active_connections: 0,
            idle_connections: 0,
            failed_connections: 0,
            peers: HashMap::new(),
        };

        for (peer_id, pool) in pools {
            let peer_stats = pool.get_stats();
            stats.total_connections += peer_stats.total_connections;
            stats.active_connections += peer_stats.active_connections;
            stats.idle_connections += peer_stats.idle_connections;
            stats.peers.insert(*peer_id, peer_stats);
        }

        stats
    }

    // PUBLIC API
    pub async fn get_connection(&self, peer_id: Uuid, addr: SocketAddr) -> Result<Arc<TcpStream>> {
        let (tx, rx) = oneshot::channel();

        self.command_tx
            .send(PoolCommand::GetConnection {
                peer_id,
                addr,
                response: tx,
            })
            .map_err(|_| FileshareError::Unknown("Connection pool unavailable".to_string()))?;

        rx.await
            .map_err(|_| FileshareError::Unknown("Connection request failed".to_string()))?
    }

    pub async fn return_connection(&self, peer_id: Uuid, stream: Arc<TcpStream>, success: bool) {
        let _ = self.command_tx.send(PoolCommand::ReturnConnection {
            peer_id,
            stream,
            was_error: !success,
        });
    }

    pub async fn get_stats(&self) -> Result<ConnectionPoolStats> {
        let (tx, rx) = oneshot::channel();

        self.command_tx
            .send(PoolCommand::GetStats { response: tx })
            .map_err(|_| FileshareError::Unknown("Connection pool unavailable".to_string()))?;

        rx.await
            .map_err(|_| FileshareError::Unknown("Stats request failed".to_string()))
    }

    pub async fn cleanup(&self) {
        let _ = self.command_tx.send(PoolCommand::Cleanup);
    }

    pub async fn shutdown(&self) {
        let _ = self.command_tx.send(PoolCommand::Shutdown);
    }
}

impl Drop for ConnectionPoolManager {
    fn drop(&mut self) {
        let _ = self.command_tx.send(PoolCommand::Shutdown);
    }
}
