use crate::{network::protocol::*, Result};
use crate::service::file_transfer::MessageSender;
use std::sync::Arc;
use tokio::sync::{Mutex, Semaphore};
use tokio::task::JoinSet;
use tracing::{debug, error, warn};
use uuid::Uuid;

pub struct ParallelChunkSender {
    message_sender: MessageSender,
    peer_id: Uuid,
    transfer_id: Uuid,
    max_parallel: usize,
    semaphore: Arc<Semaphore>,
}

impl ParallelChunkSender {
    pub fn new(
        message_sender: MessageSender,
        peer_id: Uuid,
        transfer_id: Uuid,
        max_parallel: usize,
    ) -> Self {
        Self {
            message_sender,
            peer_id,
            transfer_id,
            max_parallel,
            semaphore: Arc::new(Semaphore::new(max_parallel)),
        }
    }

    pub async fn send_chunks_parallel(
        &self,
        chunks: Vec<(u64, TransferChunk)>,
    ) -> Result<Vec<u64>> {
        let total_chunks = chunks.len();
        debug!("📤 PARALLEL_SEND: Sending {} chunks in parallel", total_chunks);
        
        let mut join_set = JoinSet::new();
        let failed_chunks = Arc::new(Mutex::new(Vec::new()));

        for (index, chunk) in chunks {
            let sender = self.message_sender.clone();
            let peer_id = self.peer_id;
            let transfer_id = self.transfer_id;
            let semaphore = self.semaphore.clone();
            let failed_chunks = failed_chunks.clone();

            join_set.spawn(async move {
                // Acquire permit for concurrency control
                let _permit = semaphore.acquire().await.unwrap();
                
                if index < 5 || index % 10 == 0 {
                    debug!("📤 Sending chunk {} in parallel", index);
                }
                
                let message = Message::new(MessageType::FileChunk { 
                    transfer_id, 
                    chunk 
                });

                if let Err(e) = sender.send((peer_id, message)) {
                    error!("❌ Failed to send chunk {}: {}", index, e);
                    failed_chunks.lock().await.push(index);
                } else if index < 5 {
                    debug!("✅ Successfully queued chunk {} for sending", index);
                }
            });
        }

        // Wait for all chunks to complete
        while let Some(result) = join_set.join_next().await {
            if let Err(e) = result {
                error!("❌ PARALLEL_SEND: Task panicked: {:?}", e);
            }
        }

        let failed_chunks_guard = failed_chunks.lock().await;
        let failed_count = failed_chunks_guard.len();
        if failed_count > 0 {
            warn!("⚠️ PARALLEL_SEND: {} out of {} chunks failed", failed_count, total_chunks);
        } else {
            debug!("✅ PARALLEL_SEND: All {} chunks sent successfully", total_chunks);
        }
        Ok(failed_chunks_guard.clone())
    }
}

pub struct ChunkBatcher {
    chunk_indices: Vec<u64>,
    batch_size: usize,
}

impl ChunkBatcher {
    pub fn new(total_chunks: u64, batch_size: usize) -> Self {
        Self {
            chunk_indices: (0..total_chunks).collect(),
            batch_size,
        }
    }

    pub fn next_batch(&mut self) -> Option<Vec<u64>> {
        if self.chunk_indices.is_empty() {
            return None;
        }

        let batch_end = self.batch_size.min(self.chunk_indices.len());
        let batch: Vec<u64> = self.chunk_indices.drain(0..batch_end).collect();
        
        Some(batch)
    }
}

// Enhanced chunk tracking for resume capability
#[derive(Debug, Clone)]
pub struct TransferTracker {
    pub transfer_id: Uuid,
    pub total_chunks: u64,
    pub completed_chunks: Vec<u64>,
    pub failed_chunks: Vec<u64>,
    pub in_progress_chunks: Vec<u64>,
}

impl TransferTracker {
    pub fn new(transfer_id: Uuid, total_chunks: u64) -> Self {
        Self {
            transfer_id,
            total_chunks,
            completed_chunks: Vec::new(),
            failed_chunks: Vec::new(),
            in_progress_chunks: Vec::new(),
        }
    }

    pub fn mark_completed(&mut self, chunk_index: u64) {
        self.completed_chunks.push(chunk_index);
        self.in_progress_chunks.retain(|&x| x != chunk_index);
    }

    pub fn mark_failed(&mut self, chunk_index: u64) {
        self.failed_chunks.push(chunk_index);
        self.in_progress_chunks.retain(|&x| x != chunk_index);
    }

    pub fn mark_in_progress(&mut self, chunk_indices: &[u64]) {
        self.in_progress_chunks.extend_from_slice(chunk_indices);
    }

    pub fn get_pending_chunks(&self) -> Vec<u64> {
        let mut pending = Vec::new();
        
        for i in 0..self.total_chunks {
            if !self.completed_chunks.contains(&i) && !self.in_progress_chunks.contains(&i) {
                pending.push(i);
            }
        }
        
        // Also retry failed chunks
        pending.extend(&self.failed_chunks);
        pending
    }

    pub fn is_complete(&self) -> bool {
        self.completed_chunks.len() == self.total_chunks as usize
    }

    pub fn progress_percentage(&self) -> f32 {
        (self.completed_chunks.len() as f32 / self.total_chunks as f32) * 100.0
    }
}