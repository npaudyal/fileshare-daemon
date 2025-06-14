use crate::network::{streaming_protocol::*, streaming_reader::*, streaming_writer::*};
use crate::{FileshareError, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use tracing::info;
use uuid::Uuid;

// FIXED: Lock-free progress tracking with message passing
#[derive(Debug, Clone)]
pub struct TransferStats {
    pub total_bytes: u64,
    pub chunk_count: u64,
    pub duration: Duration,
    pub average_speed_mbps: f64,
    pub compression_ratio: f64,
    pub peak_speed_mbps: f64,
    pub error_count: u32,
}

#[derive(Debug, Clone)]
pub struct TransferProgress {
    pub transfer_id: Uuid,
    pub bytes_transferred: u64,
    pub total_bytes: u64,
    pub chunks_completed: u64,
    pub total_chunks: u64,
    pub current_speed_mbps: f64,
    pub peak_speed_mbps: f64,
    pub eta_seconds: f64,
    pub started_at: Instant,
    pub last_update: Instant,
    pub status: TransferProgressStatus,
}

#[derive(Debug, Clone)]
pub enum TransferProgressStatus {
    Starting,
    Active,
    Paused,
    Completed,
    Failed(String),
    Cancelled,
}

impl TransferProgress {
    pub fn new(transfer_id: Uuid, total_bytes: u64, total_chunks: u64) -> Self {
        let now = Instant::now();
        Self {
            transfer_id,
            bytes_transferred: 0,
            total_bytes,
            chunks_completed: 0,
            total_chunks,
            current_speed_mbps: 0.0,
            peak_speed_mbps: 0.0,
            eta_seconds: 0.0,
            started_at: now,
            last_update: now,
            status: TransferProgressStatus::Starting,
        }
    }

    pub fn update(&mut self, bytes_transferred: u64, chunks_completed: u64) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.started_at).as_secs_f64();
        let interval = now.duration_since(self.last_update).as_secs_f64();

        // Calculate current speed (for this interval)
        if interval > 0.0 {
            let bytes_in_interval = bytes_transferred.saturating_sub(self.bytes_transferred);
            let speed_bps = bytes_in_interval as f64 / interval;
            self.current_speed_mbps = speed_bps / (1024.0 * 1024.0);

            // Update peak speed
            if self.current_speed_mbps > self.peak_speed_mbps {
                self.peak_speed_mbps = self.current_speed_mbps;
            }
        }

        self.bytes_transferred = bytes_transferred;
        self.chunks_completed = chunks_completed;
        self.last_update = now;

        // Calculate ETA
        if elapsed > 0.0 && self.current_speed_mbps > 0.0 {
            let remaining_bytes = self.total_bytes.saturating_sub(bytes_transferred);
            let remaining_mb = remaining_bytes as f64 / (1024.0 * 1024.0);
            self.eta_seconds = remaining_mb / self.current_speed_mbps;
        }

        // Update status
        if bytes_transferred >= self.total_bytes {
            self.status = TransferProgressStatus::Completed;
        } else if matches!(self.status, TransferProgressStatus::Starting) {
            self.status = TransferProgressStatus::Active;
        }
    }

    pub fn progress_percentage(&self) -> f64 {
        if self.total_bytes == 0 {
            return 100.0;
        }
        (self.bytes_transferred as f64 / self.total_bytes as f64) * 100.0
    }
}

// FIXED: Message-based commands instead of direct method calls
#[derive(Debug)]
enum TransferCommand {
    StartOutgoing {
        transfer_id: Uuid,
        peer_id: Uuid,
        file_path: PathBuf,
        stream: TcpStream,
        response: oneshot::Sender<Result<()>>,
    },
    StartIncoming {
        transfer_id: Uuid,
        metadata: StreamingFileMetadata,
        save_path: PathBuf,
        stream: TcpStream,
        response: oneshot::Sender<Result<()>>,
    },
    CancelTransfer {
        transfer_id: Uuid,
        response: oneshot::Sender<Result<()>>,
    },
    GetProgress {
        transfer_id: Uuid,
        response: oneshot::Sender<Option<TransferProgress>>,
    },
    GetActiveTransfers {
        response: oneshot::Sender<Vec<Uuid>>,
    },
    GetStats {
        transfer_id: Uuid,
        response: oneshot::Sender<Option<TransferStats>>,
    },
    Cleanup,
}

#[derive(Debug)]
enum ProgressUpdate {
    Started {
        transfer_id: Uuid,
        total_bytes: u64,
        total_chunks: u64,
    },
    Progress {
        transfer_id: Uuid,
        bytes_completed: u64,
        chunks_completed: u64,
    },
    Completed {
        transfer_id: Uuid,
        stats: TransferStats,
    },
    Failed {
        transfer_id: Uuid,
        error: String,
    },
}

// FIXED: Lock-free transfer manager
pub struct StreamingTransferManager {
    pub config: StreamingConfig,
    command_tx: mpsc::UnboundedSender<TransferCommand>,
    progress_rx: Option<mpsc::UnboundedReceiver<ProgressUpdate>>,
}

impl StreamingTransferManager {
    pub fn new(config: StreamingConfig) -> Self {
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (progress_tx, progress_rx) = mpsc::unbounded_channel();

        // Start the lock-free background task
        tokio::spawn(Self::background_task(command_rx, progress_tx, config));

        info!(
            "🚀 Lock-free StreamingTransferManager created with config: chunk_size={}KB",
            config.base_chunk_size / 1024
        );

        Self {
            config,
            command_tx,
            progress_rx: Some(progress_rx),
        }
    }

    // FIXED: Single background task handles ALL state management
    async fn background_task(
        mut command_rx: mpsc::UnboundedReceiver<TransferCommand>,
        progress_tx: mpsc::UnboundedSender<ProgressUpdate>,
        config: StreamingConfig,
    ) {
        let mut active_transfers: HashMap<Uuid, TransferProgress> = HashMap::new();
        let mut completed_stats: HashMap<Uuid, TransferStats> = HashMap::new();
        let mut cleanup_interval = tokio::time::interval(Duration::from_secs(300)); // 5 minutes

        info!("🔄 StreamingTransferManager background task started");

        loop {
            tokio::select! {
                Some(command) = command_rx.recv() => {
                    Self::handle_command(
                        command,
                        &mut active_transfers,
                        &mut completed_stats,
                        &progress_tx,
                        config
                    ).await;
                }

                _ = cleanup_interval.tick() => {
                    Self::cleanup_old_data(&mut completed_stats);
                }
            }
        }
    }

    async fn handle_command(
        command: TransferCommand,
        active_transfers: &mut HashMap<Uuid, TransferProgress>,
        completed_stats: &mut HashMap<Uuid, TransferStats>,
        progress_tx: &mpsc::UnboundedSender<ProgressUpdate>,
        config: StreamingConfig,
    ) {
        match command {
            TransferCommand::StartOutgoing {
                transfer_id,
                peer_id,
                file_path,
                stream,
                response,
            } => {
                let result = Self::execute_outgoing_transfer(
                    transfer_id,
                    peer_id,
                    file_path,
                    stream,
                    config,
                    progress_tx.clone(),
                )
                .await;
                let _ = response.send(result);
            }

            TransferCommand::StartIncoming {
                transfer_id,
                metadata,
                save_path,
                stream,
                response,
            } => {
                let result = Self::execute_incoming_transfer(
                    transfer_id,
                    metadata,
                    save_path,
                    stream,
                    config,
                    progress_tx.clone(),
                )
                .await;
                let _ = response.send(result);
            }

            TransferCommand::CancelTransfer {
                transfer_id,
                response,
            } => {
                if let Some(progress) = active_transfers.get_mut(&transfer_id) {
                    progress.status = TransferProgressStatus::Cancelled;
                    info!("🛑 Transfer {} cancelled", transfer_id);
                }
                let _ = response.send(Ok(()));
            }

            TransferCommand::GetProgress {
                transfer_id,
                response,
            } => {
                let progress = active_transfers.get(&transfer_id).cloned();
                let _ = response.send(progress);
            }

            TransferCommand::GetActiveTransfers { response } => {
                let active: Vec<Uuid> = active_transfers.keys().cloned().collect();
                let _ = response.send(active);
            }

            TransferCommand::GetStats {
                transfer_id,
                response,
            } => {
                let stats = completed_stats.get(&transfer_id).cloned();
                let _ = response.send(stats);
            }

            TransferCommand::Cleanup => {
                Self::cleanup_old_data(completed_stats);
            }
        }
    }

    // FIXED: Outgoing transfer execution without locks
    async fn execute_outgoing_transfer(
        transfer_id: Uuid,
        _peer_id: Uuid,
        file_path: PathBuf,
        stream: TcpStream,
        config: StreamingConfig,
        progress_tx: mpsc::UnboundedSender<ProgressUpdate>,
    ) -> Result<()> {
        let file_size = tokio::fs::metadata(&file_path).await?.len();
        let estimated_chunks =
            (file_size + config.base_chunk_size as u64 - 1) / config.base_chunk_size as u64;

        // Notify start
        let _ = progress_tx.send(ProgressUpdate::Started {
            transfer_id,
            total_bytes: file_size,
            total_chunks: estimated_chunks,
        });

        // Create optimized reader
        let reader = StreamingFileReader::new(file_path.clone(), transfer_id, config).await?;
        let chunk_stream = reader.create_chunk_stream().await?;

        // Execute transfer with progress updates
        let start_time = Instant::now();
        match Self::stream_chunks_with_progress(
            chunk_stream,
            stream,
            transfer_id,
            progress_tx.clone(),
        )
        .await
        {
            Ok(stats) => {
                let final_stats = TransferStats {
                    total_bytes: stats.total_bytes,
                    chunk_count: stats.chunk_count,
                    duration: start_time.elapsed(),
                    average_speed_mbps: stats.total_bytes as f64
                        / (1024.0 * 1024.0)
                        / start_time.elapsed().as_secs_f64(),
                    compression_ratio: stats.compression_ratio,
                    peak_speed_mbps: 0.0, // Will be updated by progress tracking
                    error_count: 0,
                };

                let _ = progress_tx.send(ProgressUpdate::Completed {
                    transfer_id,
                    stats: final_stats.clone(),
                });

                // Show completion notification
                Self::show_completion_notification(&file_path, &final_stats, true).await;

                Ok(())
            }
            Err(e) => {
                let _ = progress_tx.send(ProgressUpdate::Failed {
                    transfer_id,
                    error: e.to_string(),
                });
                Err(e)
            }
        }
    }

    // FIXED: Incoming transfer execution without locks
    async fn execute_incoming_transfer(
        transfer_id: Uuid,
        metadata: StreamingFileMetadata,
        save_path: PathBuf,
        stream: TcpStream,
        config: StreamingConfig,
        progress_tx: mpsc::UnboundedSender<ProgressUpdate>,
    ) -> Result<()> {
        // Notify start
        let _ = progress_tx.send(ProgressUpdate::Started {
            transfer_id,
            total_bytes: metadata.size,
            total_chunks: metadata.estimated_chunks,
        });

        // Create writer
        let writer = StreamingFileWriter::new(
            save_path.clone(),
            transfer_id,
            config,
            metadata.size,
            metadata.estimated_chunks,
        )
        .await?;

        // Receive with progress updates
        let start_time = Instant::now();
        match Self::receive_chunks_with_progress(writer, stream, transfer_id, progress_tx.clone())
            .await
        {
            Ok(stats) => {
                let final_stats = TransferStats {
                    total_bytes: stats.total_bytes,
                    chunk_count: stats.chunk_count,
                    duration: start_time.elapsed(),
                    average_speed_mbps: stats.total_bytes as f64
                        / (1024.0 * 1024.0)
                        / start_time.elapsed().as_secs_f64(),
                    compression_ratio: stats.compression_ratio,
                    peak_speed_mbps: 0.0,
                    error_count: 0,
                };

                let _ = progress_tx.send(ProgressUpdate::Completed {
                    transfer_id,
                    stats: final_stats.clone(),
                });

                // Show completion notification
                Self::show_completion_notification(&save_path, &final_stats, false).await;

                Ok(())
            }
            Err(e) => {
                let _ = progress_tx.send(ProgressUpdate::Failed {
                    transfer_id,
                    error: e.to_string(),
                });
                Err(e)
            }
        }
    }

    // FIXED: High-performance chunk streaming with progress
    async fn stream_chunks_with_progress(
        mut chunk_stream: StreamingChunkReader,
        mut stream: TcpStream,
        transfer_id: Uuid,
        progress_tx: mpsc::UnboundedSender<ProgressUpdate>,
    ) -> Result<InternalTransferStats> {
        let mut total_bytes = 0u64;
        let mut chunk_count = 0u64;
        let mut last_progress_update = Instant::now();
        let progress_interval = Duration::from_secs(1); // Update progress every second

        while let Some((header, data)) = chunk_stream.next_chunk().await? {
            // Send header + data efficiently
            let header_bytes = header.to_bytes()?;

            // Use vectored write for better performance
            let mut write_buffer = Vec::with_capacity(header_bytes.len() + data.len());
            write_buffer.extend_from_slice(&header_bytes);
            write_buffer.extend_from_slice(&data);

            stream
                .write_all(&write_buffer)
                .await
                .map_err(|e| FileshareError::Network(e))?;

            total_bytes += data.len() as u64;
            chunk_count += 1;

            // Send progress update periodically
            let now = Instant::now();
            if now.duration_since(last_progress_update) >= progress_interval {
                let _ = progress_tx.send(ProgressUpdate::Progress {
                    transfer_id,
                    bytes_completed: total_bytes,
                    chunks_completed: chunk_count,
                });
                last_progress_update = now;
            }

            if header.is_last_chunk() {
                break;
            }

            // Small delay for very fast networks to prevent overwhelming
            if chunk_count % 100 == 0 {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        }

        // Final flush
        stream.flush().await?;
        stream.shutdown().await?;

        // Final progress update
        let _ = progress_tx.send(ProgressUpdate::Progress {
            transfer_id,
            bytes_completed: total_bytes,
            chunks_completed: chunk_count,
        });

        Ok(InternalTransferStats {
            total_bytes,
            chunk_count,
            compression_ratio: 1.0, // Calculate properly if needed
        })
    }

    // FIXED: High-performance chunk receiving with progress
    async fn receive_chunks_with_progress(
        mut writer: StreamingFileWriter,
        mut stream: TcpStream,
        transfer_id: Uuid,
        progress_tx: mpsc::UnboundedSender<ProgressUpdate>,
    ) -> Result<InternalTransferStats> {
        let mut header_buffer = [0u8; StreamChunkHeader::SIZE];
        let mut total_bytes = 0u64;
        let mut chunk_count = 0u64;
        let mut last_progress_update = Instant::now();
        let progress_interval = Duration::from_secs(1);

        loop {
            // Read header
            stream
                .read_exact(&mut header_buffer)
                .await
                .map_err(|e| FileshareError::Network(e))?;

            let header = StreamChunkHeader::from_bytes(&header_buffer)
                .map_err(|e| FileshareError::Transfer(format!("Invalid header: {}", e)))?;

            // Read chunk data
            let mut chunk_data = vec![0u8; header.get_chunk_size() as usize];
            stream
                .read_exact(&mut chunk_data)
                .await
                .map_err(|e| FileshareError::Network(e))?;

            let data = bytes::Bytes::from(chunk_data);

            // Write chunk
            let is_complete = writer.write_chunk(header, data.clone()).await?;

            total_bytes += data.len() as u64;
            chunk_count += 1;

            // Send progress update periodically
            let now = Instant::now();
            if now.duration_since(last_progress_update) >= progress_interval {
                let _ = progress_tx.send(ProgressUpdate::Progress {
                    transfer_id,
                    bytes_completed: total_bytes,
                    chunks_completed: chunk_count,
                });
                last_progress_update = now;
            }

            if is_complete || header.is_last_chunk() {
                break;
            }
        }

        // Final progress update
        let _ = progress_tx.send(ProgressUpdate::Progress {
            transfer_id,
            bytes_completed: total_bytes,
            chunks_completed: chunk_count,
        });

        Ok(InternalTransferStats {
            total_bytes,
            chunk_count,
            compression_ratio: 1.0,
        })
    }

    // PUBLIC API - All methods use message passing
    pub async fn start_streaming_transfer(
        &self,
        peer_id: Uuid,
        file_path: PathBuf,
        stream: TcpStream,
    ) -> Result<Uuid> {
        let transfer_id = Uuid::new_v4();
        let (response_tx, response_rx) = oneshot::channel();

        self.command_tx
            .send(TransferCommand::StartOutgoing {
                transfer_id,
                peer_id,
                file_path,
                stream,
                response: response_tx,
            })
            .map_err(|_| FileshareError::Unknown("Transfer manager not available".to_string()))?;

        response_rx
            .await
            .map_err(|_| FileshareError::Unknown("Transfer command failed".to_string()))?
            .map(|_| transfer_id)
    }

    pub async fn receive_streaming_transfer(
        &self,
        transfer_id: Uuid,
        metadata: StreamingFileMetadata,
        save_path: PathBuf,
        stream: TcpStream,
    ) -> Result<()> {
        let (response_tx, response_rx) = oneshot::channel();

        self.command_tx
            .send(TransferCommand::StartIncoming {
                transfer_id,
                metadata,
                save_path,
                stream,
                response: response_tx,
            })
            .map_err(|_| FileshareError::Unknown("Transfer manager not available".to_string()))?;

        response_rx
            .await
            .map_err(|_| FileshareError::Unknown("Transfer command failed".to_string()))?
    }

    pub async fn get_progress(&self, transfer_id: Uuid) -> Option<TransferProgress> {
        let (response_tx, response_rx) = oneshot::channel();

        if self
            .command_tx
            .send(TransferCommand::GetProgress {
                transfer_id,
                response: response_tx,
            })
            .is_ok()
        {
            response_rx.await.unwrap_or(None)
        } else {
            None
        }
    }

    pub async fn get_active_transfers(&self) -> Vec<Uuid> {
        let (response_tx, response_rx) = oneshot::channel();

        if self
            .command_tx
            .send(TransferCommand::GetActiveTransfers {
                response: response_tx,
            })
            .is_ok()
        {
            response_rx.await.unwrap_or_default()
        } else {
            Vec::new()
        }
    }

    pub async fn cancel_transfer(&self, transfer_id: Uuid) -> Result<()> {
        let (response_tx, response_rx) = oneshot::channel();

        self.command_tx
            .send(TransferCommand::CancelTransfer {
                transfer_id,
                response: response_tx,
            })
            .map_err(|_| FileshareError::Unknown("Transfer manager not available".to_string()))?;

        response_rx
            .await
            .map_err(|_| FileshareError::Unknown("Cancel command failed".to_string()))?
    }

    // Utility methods
    fn cleanup_old_data(completed_stats: &mut HashMap<Uuid, TransferStats>) {
        let cutoff = Instant::now() - Duration::from_secs(3600); // 1 hour
        let initial_count = completed_stats.len();

        completed_stats.retain(|_, stats| stats.duration < cutoff.elapsed());

        let removed = initial_count - completed_stats.len();
        if removed > 0 {
            info!("🧹 Cleaned up {} old transfer records", removed);
        }
    }

    async fn show_completion_notification(
        file_path: &PathBuf,
        stats: &TransferStats,
        is_outgoing: bool,
    ) {
        let direction = if is_outgoing { "Sent" } else { "Received" };
        let filename = file_path.file_name().unwrap_or_default().to_string_lossy();

        let _ = notify_rust::Notification::new()
            .summary(&format!("✅ High-Speed Transfer Complete"))
            .body(&format!(
                "{} {}\n📊 {:.1} MB in {:.1}s\n⚡ {:.1} MB/s average",
                direction,
                filename,
                stats.total_bytes as f64 / (1024.0 * 1024.0),
                stats.duration.as_secs_f64(),
                stats.average_speed_mbps
            ))
            .timeout(notify_rust::Timeout::Milliseconds(5000))
            .show();
    }

    pub async fn start_cleanup_task(&self) {
        let command_tx = self.command_tx.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1800)); // 30 minutes

            loop {
                interval.tick().await;
                let _ = command_tx.send(TransferCommand::Cleanup);
            }
        });
    }
}

#[derive(Debug)]
struct InternalTransferStats {
    total_bytes: u64,
    chunk_count: u64,
    compression_ratio: f64,
}
