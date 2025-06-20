use std::time::{Duration, Instant};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct TransferProgress {
    pub transfer_id: Uuid,
    pub file_name: String,
    pub total_size: u64,
    pub bytes_transferred: u64,
    pub chunks_completed: u64,
    pub total_chunks: u64,
    pub start_time: Instant,
    pub last_update: Instant,
    pub current_speed: f64,       // bytes per second
    pub average_speed: f64,       // overall average
    pub eta_seconds: Option<u64>, // estimated time remaining
    pub is_streaming: bool,
}

impl TransferProgress {
    pub fn new(transfer_id: Uuid, file_name: String, total_size: u64, total_chunks: u64) -> Self {
        let now = Instant::now();
        Self {
            transfer_id,
            file_name,
            total_size,
            bytes_transferred: 0,
            chunks_completed: 0,
            total_chunks,
            start_time: now,
            last_update: now,
            current_speed: 0.0,
            average_speed: 0.0,
            eta_seconds: None,
            is_streaming: true,
        }
    }

    pub fn update(&mut self, bytes_transferred: u64, chunks_completed: u64) {
        let now = Instant::now();
        let time_elapsed = now.duration_since(self.last_update).as_secs_f64();

        if time_elapsed > 0.0 {
            let bytes_delta = bytes_transferred.saturating_sub(self.bytes_transferred);
            self.current_speed = bytes_delta as f64 / time_elapsed;
        }

        self.bytes_transferred = bytes_transferred;
        self.chunks_completed = chunks_completed;
        self.last_update = now;

        let total_elapsed = now.duration_since(self.start_time).as_secs_f64();
        if total_elapsed > 0.0 {
            self.average_speed = self.bytes_transferred as f64 / total_elapsed;

            let remaining_bytes = self.total_size.saturating_sub(self.bytes_transferred);
            if self.average_speed > 0.0 {
                self.eta_seconds = Some((remaining_bytes as f64 / self.average_speed) as u64);
            }
        }
    }

    pub fn progress_percentage(&self) -> f64 {
        if self.total_size == 0 {
            return 100.0;
        }
        (self.bytes_transferred as f64 / self.total_size as f64) * 100.0
    }

    pub fn format_speed(&self) -> String {
        Self::format_bytes_per_second(self.current_speed as u64)
    }

    pub fn format_average_speed(&self) -> String {
        Self::format_bytes_per_second(self.average_speed as u64)
    }

    pub fn format_eta(&self) -> String {
        match self.eta_seconds {
            None => "Unknown".to_string(),
            Some(seconds) => {
                if seconds < 60 {
                    format!("{}s", seconds)
                } else if seconds < 3600 {
                    format!("{}m {}s", seconds / 60, seconds % 60)
                } else {
                    let hours = seconds / 3600;
                    let minutes = (seconds % 3600) / 60;
                    format!("{}h {}m", hours, minutes)
                }
            }
        }
    }

    fn format_bytes_per_second(bytes_per_second: u64) -> String {
        Self::format_bytes(bytes_per_second) + "/s"
    }

    fn format_bytes(bytes: u64) -> String {
        if bytes < 1024 {
            format!("{} B", bytes)
        } else if bytes < 1024 * 1024 {
            format!("{:.1} KB", bytes as f64 / 1024.0)
        } else if bytes < 1024 * 1024 * 1024 {
            format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
        } else {
            format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
        }
    }
}
