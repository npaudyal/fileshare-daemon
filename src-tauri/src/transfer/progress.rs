use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::time::Instant;

const SPEED_SAMPLE_WINDOW_SIZE: usize = 10;

#[derive(Debug, Clone)]
pub struct TransferProgress {
    total_bytes: u64,
    transferred_bytes: u64,
    start_time: Instant,
    last_update: Instant,
    speed_samples: VecDeque<SpeedSample>,
}

#[derive(Debug, Clone)]
struct SpeedSample {
    bytes: u64,
    timestamp: Instant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressInfo {
    pub total_bytes: u64,
    pub transferred_bytes: u64,
    pub progress_percentage: f32,
    pub speed_bps: u64,
    pub time_remaining_seconds: Option<u64>,
    pub elapsed_seconds: u64,
}

impl TransferProgress {
    pub fn new(total_bytes: u64) -> Self {
        let now = Instant::now();
        Self {
            total_bytes,
            transferred_bytes: 0,
            start_time: now,
            last_update: now,
            speed_samples: VecDeque::with_capacity(SPEED_SAMPLE_WINDOW_SIZE),
        }
    }

    pub fn update(&mut self, bytes_transferred: u64) {
        let now = Instant::now();
        self.transferred_bytes = bytes_transferred;

        // Add speed sample
        self.speed_samples.push_back(SpeedSample {
            bytes: bytes_transferred,
            timestamp: now,
        });

        // Keep only recent samples
        while self.speed_samples.len() > SPEED_SAMPLE_WINDOW_SIZE {
            self.speed_samples.pop_front();
        }

        self.last_update = now;
    }

    pub fn add_bytes(&mut self, bytes: u64) {
        self.update(self.transferred_bytes + bytes);
    }

    pub fn get_info(&self) -> ProgressInfo {
        let elapsed = self.start_time.elapsed();
        let elapsed_seconds = elapsed.as_secs();

        let progress_percentage = if self.total_bytes > 0 {
            (self.transferred_bytes as f32 / self.total_bytes as f32) * 100.0
        } else {
            0.0
        };

        // Calculate average speed from samples
        let speed_bps = self.calculate_average_speed();

        // Calculate time remaining
        let time_remaining_seconds = if speed_bps > 0 && self.transferred_bytes < self.total_bytes {
            let remaining_bytes = self.total_bytes - self.transferred_bytes;
            Some(remaining_bytes / speed_bps)
        } else {
            None
        };

        ProgressInfo {
            total_bytes: self.total_bytes,
            transferred_bytes: self.transferred_bytes,
            progress_percentage,
            speed_bps,
            time_remaining_seconds,
            elapsed_seconds,
        }
    }

    fn calculate_average_speed(&self) -> u64 {
        if self.speed_samples.len() < 2 {
            // Not enough samples for speed calculation
            if self.transferred_bytes > 0 && self.start_time.elapsed().as_secs() > 0 {
                return self.transferred_bytes / self.start_time.elapsed().as_secs();
            }
            return 0;
        }

        let first = &self.speed_samples[0];
        let last = &self.speed_samples[self.speed_samples.len() - 1];

        let time_diff = last.timestamp.duration_since(first.timestamp);
        let bytes_diff = last.bytes.saturating_sub(first.bytes);

        if time_diff.as_secs() > 0 {
            bytes_diff / time_diff.as_secs()
        } else if time_diff.as_millis() > 0 {
            (bytes_diff * 1000) / time_diff.as_millis() as u64
        } else {
            0
        }
    }

    pub fn is_complete(&self) -> bool {
        self.transferred_bytes >= self.total_bytes
    }

    pub fn reset(&mut self) {
        let now = Instant::now();
        self.transferred_bytes = 0;
        self.start_time = now;
        self.last_update = now;
        self.speed_samples.clear();
    }
}