// src/network/high_speed_pipeline.rs - FIXED VERSION
use super::high_speed_streaming::*;
use crate::network::high_speed_memory::HighSpeedMemoryPool;
use crate::network::performance_monitor::PerformanceMonitor;
use crate::{FileshareError, Result};
use bytes::{Bytes, BytesMut};
use lz4_flex::compress_prepend_size;
use memmap2::{Mmap, MmapOptions};
use std::collections::VecDeque;
use std::fs::File;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

impl HighSpeedStreamingManager {
    // FIXED: Start the complete outgoing pipeline with proper channel architecture
    pub async fn start_outgoing_pipeline(
        transfer_id: uuid::Uuid,
        file_path: PathBuf,
        file_size: u64,
        connections: Vec<HighSpeedConnection>,
        memory_pool: Arc<HighSpeedMemoryPool>,
        performance_monitor: Arc<PerformanceMonitor>,
        config: HighSpeedConfig,
        cancel_token: tokio_util::sync::CancellationToken,
    ) -> Result<Vec<tokio::task::JoinHandle<()>>> {
        info!(
            "🔄 Starting high-speed pipeline for transfer {}",
            transfer_id
        );

        // Create pipeline channels - FIXED: No cloning of receivers
        let (read_tx, read_rx) = mpsc::channel::<RawChunk>(config.pipeline_depth);

        // Create multiple processor channels instead of cloning receivers
        let processor_count = 4;
        let mut processor_channels = Vec::new();

        for _ in 0..processor_count {
            let (process_tx, process_rx) = mpsc::channel::<ProcessedChunk>(config.pipeline_depth);
            processor_channels.push((process_tx, process_rx));
        }

        let (batch_tx, batch_rx) = mpsc::channel::<NetworkBatch>(config.pipeline_depth / 2);

        let mut handles = Vec::new();

        // Stage 1: High-speed file reader
        let reader_handle = Self::start_file_reader_stage(
            transfer_id,
            file_path,
            file_size,
            config.clone(),
            memory_pool.clone(),
            read_tx,
            processor_channels
                .iter()
                .map(|(tx, _)| tx.clone())
                .collect(),
            cancel_token.clone(),
        );
        handles.push(reader_handle);

        // Stage 2: Multiple chunk processors - FIXED: Each gets its own receiver
        for (processor_id, (_, process_rx)) in processor_channels.into_iter().enumerate() {
            let processor_handle = Self::start_chunk_processor_stage(
                processor_id,
                config.clone(),
                memory_pool.clone(),
                process_rx, // Each processor gets its own receiver
                batch_tx.clone(),
                cancel_token.clone(),
            );
            handles.push(processor_handle);
        }

        // Stage 3: Parallel network senders - FIXED: Use broadcast for multiple senders
        let connection_count = connections.len();
        let (broadcast_tx, _) =
            tokio::sync::broadcast::channel::<NetworkBatch>(config.pipeline_depth);

        // Batch distributor - receives from processors and broadcasts to senders
        let distributor_handle = Self::start_batch_distributor_stage(
            batch_rx,
            broadcast_tx.clone(),
            cancel_token.clone(),
        );
        handles.push(distributor_handle);

        // Stage 4: Parallel network senders
        for (connection_id, connection) in connections.into_iter().enumerate() {
            let sender_handle = Self::start_network_sender_stage(
                connection_id,
                connection,
                broadcast_tx.subscribe(), // Each sender gets its own subscription
                performance_monitor.clone(),
                connection_count,
                cancel_token.clone(),
            );
            handles.push(sender_handle);
        }

        info!(
            "✅ High-speed pipeline started with {} stages",
            handles.len()
        );
        Ok(handles)
    }

    // FIXED: File reader stage with proper distribution to processors
    fn start_file_reader_stage(
        transfer_id: uuid::Uuid,
        file_path: PathBuf,
        file_size: u64,
        config: HighSpeedConfig,
        memory_pool: Arc<HighSpeedMemoryPool>,
        read_tx: mpsc::Sender<RawChunk>,
        processor_txs: Vec<mpsc::Sender<ProcessedChunk>>, // Direct to processors
        cancel_token: tokio_util::sync::CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            info!(
                "📖 Starting high-speed file reader for {}",
                file_path.display()
            );

            let result = Self::execute_file_reader_fixed(
                transfer_id,
                file_path,
                file_size,
                config,
                memory_pool,
                processor_txs,
                cancel_token,
            )
            .await;

            if let Err(e) = result {
                warn!("❌ File reader stage failed: {}", e);
            }
        })
    }

    // FIXED: File reader that directly distributes to processors
    async fn execute_file_reader_fixed(
        _transfer_id: uuid::Uuid,
        file_path: PathBuf,
        file_size: u64,
        config: HighSpeedConfig,
        memory_pool: Arc<HighSpeedMemoryPool>,
        processor_txs: Vec<mpsc::Sender<ProcessedChunk>>,
        cancel_token: tokio_util::sync::CancellationToken,
    ) -> Result<()> {
        // Decide between memory mapping and buffered I/O
        let use_mmap = file_size > 50 * 1024 * 1024; // Use mmap for files > 50MB

        if use_mmap {
            Self::execute_mmap_reader_fixed(
                file_path,
                file_size,
                config,
                processor_txs,
                cancel_token,
            )
            .await
        } else {
            Self::execute_buffered_reader_fixed(
                file_path,
                file_size,
                config,
                memory_pool,
                processor_txs,
                cancel_token,
            )
            .await
        }
    }

    // FIXED: Memory-mapped file reader with direct processing
    async fn execute_mmap_reader_fixed(
        file_path: PathBuf,
        file_size: u64,
        config: HighSpeedConfig,
        processor_txs: Vec<mpsc::Sender<ProcessedChunk>>,
        cancel_token: tokio_util::sync::CancellationToken,
    ) -> Result<()> {
        let file = File::open(&file_path)?;

        // Create optimized memory mapping
        let mmap = unsafe {
            let mut options = MmapOptions::new();

            #[cfg(target_os = "linux")]
            {
                if let Err(e) = options.huge(Some(memmap2::Advice::Hugepage)) {
                    debug!("Could not enable huge pages: {}", e);
                }
                options.populate();
            }

            options.map(&file)?
        };

        info!("🗄️ Memory mapped {} bytes", mmap.len());

        let chunk_size = config.chunk_size;
        let mut current_offset = 0;
        let mut chunk_index = 0u64;
        let processor_count = processor_txs.len();

        // Read and process chunks at maximum speed
        while current_offset < file_size as usize {
            if cancel_token.is_cancelled() {
                break;
            }

            let chunk_end = std::cmp::min(current_offset + chunk_size, mmap.len());
            let chunk_data = &mmap[current_offset..chunk_end];
            let is_last = chunk_end >= mmap.len();

            // FIXED: Process chunk inline to avoid borrow issues
            let processed_chunk =
                Self::process_chunk_inline(chunk_index, chunk_data, is_last, &config).await?;

            // Round-robin distribution to processors
            let processor_index = (chunk_index as usize) % processor_count;
            if let Some(processor_tx) = processor_txs.get(processor_index) {
                if processor_tx.send(processed_chunk).await.is_err() {
                    break; // Channel closed
                }
            }

            current_offset = chunk_end;
            chunk_index += 1;

            // High-speed reading - minimal logging
            if chunk_index % 200 == 0 {
                debug!(
                    "📖 Processed {} chunks ({:.1}%)",
                    chunk_index,
                    (current_offset as f64 / mmap.len() as f64) * 100.0
                );
            }
        }

        info!("✅ File reader completed: {} chunks processed", chunk_index);
        Ok(())
    }

    // FIXED: Buffered reader with direct processing
    async fn execute_buffered_reader_fixed(
        file_path: PathBuf,
        _file_size: u64,
        config: HighSpeedConfig,
        memory_pool: Arc<HighSpeedMemoryPool>,
        processor_txs: Vec<mpsc::Sender<ProcessedChunk>>,
        cancel_token: tokio_util::sync::CancellationToken,
    ) -> Result<()> {
        use tokio::io::AsyncReadExt;

        let mut file = tokio::fs::File::open(&file_path).await?;
        let chunk_size = config.chunk_size;
        let mut chunk_index = 0u64;
        let processor_count = processor_txs.len();

        loop {
            if cancel_token.is_cancelled() {
                break;
            }

            // Get buffer from pool
            let mut buffer = memory_pool.get_chunk_buffer().await;
            buffer.resize(chunk_size, 0);

            // Read chunk
            let bytes_read = file.read(&mut buffer).await?;
            if bytes_read == 0 {
                break; // EOF
            }

            buffer.truncate(bytes_read);
            let is_last = bytes_read < chunk_size;

            // FIXED: Process chunk inline
            let processed_chunk =
                Self::process_chunk_inline(chunk_index, &buffer, is_last, &config).await?;

            // Round-robin distribution
            let processor_index = (chunk_index as usize) % processor_count;
            if let Some(processor_tx) = processor_txs.get(processor_index) {
                if processor_tx.send(processed_chunk).await.is_err() {
                    break;
                }
            }

            chunk_index += 1;

            // Return buffer to pool
            memory_pool.return_chunk_buffer(buffer).await;
        }

        info!("✅ Buffered reader completed: {} chunks", chunk_index);
        Ok(())
    }

    // FIXED: Inline chunk processing to avoid borrow issues
    async fn process_chunk_inline(
        chunk_index: u64,
        chunk_data: &[u8],
        is_last: bool,
        config: &HighSpeedConfig,
    ) -> Result<ProcessedChunk> {
        let process_start = Instant::now();

        // Smart compression decision
        let should_compress = chunk_data.len() > 4096 // Only compress chunks > 4KB
            && Self::should_compress_content(chunk_data);

        let (final_data, compressed, original_size) = if should_compress {
            // Fast LZ4 compression
            let data_to_compress = chunk_data.to_vec();
            let compressed_data =
                tokio::task::spawn_blocking(move || compress_prepend_size(&data_to_compress))
                    .await
                    .map_err(|e| {
                        FileshareError::Unknown(format!("Compression task failed: {}", e))
                    })?;

            // Only use compressed data if it's actually smaller
            if compressed_data.len() < chunk_data.len() {
                (Bytes::from(compressed_data), true, chunk_data.len())
            } else {
                (Bytes::copy_from_slice(chunk_data), false, chunk_data.len())
            }
        } else {
            (Bytes::copy_from_slice(chunk_data), false, chunk_data.len())
        };

        Ok(ProcessedChunk {
            chunk_index,
            data: final_data,
            compressed,
            original_size,
            process_timestamp: process_start,
        })
    }

    // FIXED: Individual chunk processor stage (simplified)
    fn start_chunk_processor_stage(
        processor_id: usize,
        config: HighSpeedConfig,
        _memory_pool: Arc<HighSpeedMemoryPool>,
        mut process_rx: mpsc::Receiver<ProcessedChunk>,
        batch_tx: mpsc::Sender<NetworkBatch>,
        cancel_token: tokio_util::sync::CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            info!("⚙️ Starting batch assembler {}", processor_id);

            let mut current_batch: Vec<ProcessedChunk> = Vec::new();
            let mut current_batch_size = 0usize;
            let mut batch_id = 0u64;

            while let Some(processed_chunk) = process_rx.recv().await {
                if cancel_token.is_cancelled() {
                    break;
                }

                current_batch_size += processed_chunk.data.len();
                current_batch.push(processed_chunk);

                // Send batch when it reaches target size
                let should_send_batch =
                    current_batch_size >= config.batch_size || current_batch.len() >= 8;

                if should_send_batch && !current_batch.is_empty() {
                    let mut batch_header = HighSpeedBatchHeader::new(
                        batch_id,
                        current_batch.len() as u16,
                        current_batch_size as u32,
                    );

                    // Check if any chunk in batch is compressed
                    if current_batch.iter().any(|c| c.compressed) {
                        batch_header.set_compressed();
                    }

                    let network_batch = NetworkBatch {
                        batch_id,
                        chunks: std::mem::take(&mut current_batch),
                        total_size: current_batch_size,
                        connection_id: processor_id, // Use processor ID for connection assignment
                        batch_header,
                    };

                    if batch_tx.send(network_batch).await.is_err() {
                        break;
                    }

                    current_batch_size = 0;
                    batch_id += 1;
                }
            }

            // Send final batch if any
            if !current_batch.is_empty() {
                let mut batch_header = HighSpeedBatchHeader::new(
                    batch_id,
                    current_batch.len() as u16,
                    current_batch_size as u32,
                );
                batch_header.set_last_batch();

                let final_batch = NetworkBatch {
                    batch_id,
                    chunks: current_batch,
                    total_size: current_batch_size,
                    connection_id: processor_id,
                    batch_header,
                };

                let _ = batch_tx.send(final_batch).await;
            }

            info!("✅ Batch assembler {} completed", processor_id);
        })
    }

    // NEW: Batch distributor that broadcasts to all network senders
    fn start_batch_distributor_stage(
        mut batch_rx: mpsc::Receiver<NetworkBatch>,
        broadcast_tx: tokio::sync::broadcast::Sender<NetworkBatch>,
        cancel_token: tokio_util::sync::CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            info!("📡 Starting batch distributor");

            let mut batches_distributed = 0u64;

            while let Some(batch) = batch_rx.recv().await {
                if cancel_token.is_cancelled() {
                    break;
                }

                // Broadcast batch to all network senders
                if let Err(e) = broadcast_tx.send(batch) {
                    warn!("Failed to broadcast batch: {}", e);
                    break;
                }

                batches_distributed += 1;

                if batches_distributed % 100 == 0 {
                    debug!("📡 Distributed {} batches", batches_distributed);
                }
            }

            info!(
                "✅ Batch distributor completed: {} batches",
                batches_distributed
            );
        })
    }

    // FIXED: Network sender stage with broadcast receiver
    fn start_network_sender_stage(
        connection_id: usize,
        mut connection: HighSpeedConnection,
        mut batch_rx: tokio::sync::broadcast::Receiver<NetworkBatch>,
        performance_monitor: Arc<PerformanceMonitor>,
        total_connections: usize,
        cancel_token: tokio_util::sync::CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            info!("📡 Starting network sender {}", connection_id);

            let mut batches_sent = 0u64;
            let mut bytes_sent = 0u64;

            while let Ok(batch) = batch_rx.recv().await {
                if cancel_token.is_cancelled() {
                    break;
                }

                // Use round-robin assignment based on batch_id
                if (batch.batch_id as usize) % total_connections != connection_id {
                    continue; // This batch is for another connection
                }

                let send_result = Self::send_batch_optimized(&mut connection.stream, batch).await;

                match send_result {
                    Ok(sent_bytes) => {
                        bytes_sent += sent_bytes;
                        batches_sent += 1;
                        connection.bytes_sent.store(bytes_sent, Ordering::Relaxed);

                        // Update performance metrics
                        performance_monitor
                            .record_bytes_sent(connection_id, sent_bytes)
                            .await;

                        if batches_sent % 50 == 0 {
                            debug!(
                                "📡 Connection {} sent {} batches ({:.1}MB)",
                                connection_id,
                                batches_sent,
                                bytes_sent as f64 / (1024.0 * 1024.0)
                            );
                        }
                    }
                    Err(e) => {
                        warn!(
                            "❌ Network send failed on connection {}: {}",
                            connection_id, e
                        );
                        break;
                    }
                }
            }

            info!(
                "✅ Network sender {} completed: {} batches, {:.1}MB",
                connection_id,
                batches_sent,
                bytes_sent as f64 / (1024.0 * 1024.0)
            );
        })
    }

    // Keep existing optimized batch sending method
    async fn send_batch_optimized(
        stream: &mut tokio::net::TcpStream,
        batch: NetworkBatch,
    ) -> Result<u64> {
        let mut total_sent = 0u64;

        // Send batch header first
        let header_bytes = batch.batch_header.to_bytes();
        stream.write_all(&header_bytes).await?;
        total_sent += header_bytes.len() as u64;

        // Send all chunks in the batch with minimal syscalls
        let mut batch_data = Vec::with_capacity(batch.total_size);

        for chunk in &batch.chunks {
            // Add chunk metadata (8 bytes: 4 bytes size + 4 bytes flags)
            let chunk_size = chunk.data.len() as u32;
            let chunk_flags = if chunk.compressed { 1u32 } else { 0u32 };

            batch_data.extend_from_slice(&chunk_size.to_be_bytes());
            batch_data.extend_from_slice(&chunk_flags.to_be_bytes());
            batch_data.extend_from_slice(&chunk.data);
        }

        // Single write for entire batch
        stream.write_all(&batch_data).await?;
        total_sent += batch_data.len() as u64;

        // Flush periodically for better throughput
        if batch.batch_id % 4 == 0 {
            stream.flush().await?;
        }

        Ok(total_sent)
    }

    // Keep existing content compression heuristic
    fn should_compress_content(data: &[u8]) -> bool {
        if data.len() < 1024 {
            return false;
        }

        // Sample first 512 bytes to check entropy
        let sample_size = std::cmp::min(512, data.len());
        let sample = &data[..sample_size];

        // Simple entropy check - count unique bytes
        let mut byte_counts = [0u32; 256];
        for &byte in sample {
            byte_counts[byte as usize] += 1;
        }

        let unique_bytes = byte_counts.iter().filter(|&&count| count > 0).count();
        let entropy_ratio = unique_bytes as f64 / 256.0;

        // Compress if entropy is low (< 0.8 means good compression potential)
        entropy_ratio < 0.8
    }
}
