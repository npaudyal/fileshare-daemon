use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Moving average calculator for smooth speed calculations
#[derive(Debug, Clone, Default)]
pub struct MovingAverage {
    samples: VecDeque<f64>,
    max_samples: usize,
    sum: f64,
}

impl MovingAverage {
    pub fn new(max_samples: usize) -> Self {
        Self {
            samples: VecDeque::with_capacity(max_samples),
            max_samples,
            sum: 0.0,
        }
    }

    pub fn add_sample(&mut self, value: f64) {
        if self.samples.len() >= self.max_samples {
            if let Some(old) = self.samples.pop_front() {
                self.sum -= old;
            }
        }
        self.samples.push_back(value);
        self.sum += value;
    }

    pub fn average(&self) -> f64 {
        if self.samples.is_empty() {
            return 0.0;
        }
        self.sum / self.samples.len() as f64
    }

    pub fn clear(&mut self) {
        self.samples.clear();
        self.sum = 0.0;
    }
}

/// Progress tracker for individual transfers
#[derive(Debug, Clone)]
pub struct ProgressTracker {
    pub total_bytes: u64,
    pub transferred_bytes: u64,
    pub start_time: Instant,
    pub last_update: Instant,
    pub last_bytes: u64,
    speed_calculator: MovingAverage,
    update_interval: Duration,
}

impl ProgressTracker {
    pub fn new(total_bytes: u64) -> Self {
        let now = Instant::now();
        Self {
            total_bytes,
            transferred_bytes: 0,
            start_time: now,
            last_update: now,
            last_bytes: 0,
            speed_calculator: MovingAverage::new(10),
            update_interval: Duration::from_millis(100), // Update every 100ms
        }
    }

    pub fn update(&mut self, bytes: u64) -> Option<ProgressUpdate> {
        self.transferred_bytes = bytes;
        let now = Instant::now();

        // Only calculate speed if enough time has passed
        if now.duration_since(self.last_update) >= self.update_interval {
            let time_delta = now.duration_since(self.last_update).as_secs_f64();
            let bytes_delta = (bytes - self.last_bytes) as f64;

            if time_delta > 0.0 {
                let speed = bytes_delta / time_delta;
                self.speed_calculator.add_sample(speed);
            }

            self.last_update = now;
            self.last_bytes = bytes;

            return Some(self.get_update());
        }

        None
    }

    pub fn get_update(&self) -> ProgressUpdate {
        let progress_percentage = if self.total_bytes > 0 {
            (self.transferred_bytes as f32 / self.total_bytes as f32) * 100.0
        } else {
            0.0
        };

        let speed = self.speed_calculator.average() as u64;
        let eta = if speed > 0 && self.transferred_bytes < self.total_bytes {
            let remaining = self.total_bytes - self.transferred_bytes;
            Some(Duration::from_secs(remaining / speed))
        } else {
            None
        };

        let elapsed = Instant::now().duration_since(self.start_time);

        ProgressUpdate {
            transferred_bytes: self.transferred_bytes,
            total_bytes: self.total_bytes,
            progress_percentage,
            speed,
            eta,
            elapsed,
        }
    }

    pub fn reset(&mut self) {
        let now = Instant::now();
        self.transferred_bytes = 0;
        self.start_time = now;
        self.last_update = now;
        self.last_bytes = 0;
        self.speed_calculator.clear();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressUpdate {
    pub transferred_bytes: u64,
    pub total_bytes: u64,
    pub progress_percentage: f32,
    pub speed: u64, // bytes per second
    pub eta: Option<Duration>,
    pub elapsed: Duration,
}

/// Global transfer metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferMetrics {
    pub total_transfers: usize,
    pub active_transfers: usize,
    pub completed_transfers: usize,
    pub failed_transfers: usize,
    pub total_bytes_transferred: u64,
    pub current_bandwidth_usage: u64,
    pub peak_bandwidth_usage: u64,
    pub average_speed: u64,
    #[serde(skip, default = "Instant::now")]
    pub session_started: Instant,
}

impl Default for TransferMetrics {
    fn default() -> Self {
        Self {
            total_transfers: 0,
            active_transfers: 0,
            completed_transfers: 0,
            failed_transfers: 0,
            total_bytes_transferred: 0,
            current_bandwidth_usage: 0,
            peak_bandwidth_usage: 0,
            average_speed: 0,
            session_started: Instant::now(),
        }
    }
}

impl TransferMetrics {
    pub fn update_bandwidth(&mut self, current: u64) {
        self.current_bandwidth_usage = current;
        if current > self.peak_bandwidth_usage {
            self.peak_bandwidth_usage = current;
        }
    }

    pub fn transfer_completed(&mut self, bytes: u64) {
        self.completed_transfers += 1;
        self.total_bytes_transferred += bytes;
        if self.active_transfers > 0 {
            self.active_transfers -= 1;
        }
    }

    pub fn transfer_failed(&mut self) {
        self.failed_transfers += 1;
        if self.active_transfers > 0 {
            self.active_transfers -= 1;
        }
    }

    pub fn transfer_started(&mut self) {
        self.total_transfers += 1;
        self.active_transfers += 1;
    }
}