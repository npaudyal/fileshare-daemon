pub mod manager;
pub mod progress;
pub mod state;

pub use manager::{TransferManager, TransferInfo, TransferDetails};
pub use progress::{ProgressTracker, MovingAverage, TransferMetrics};
pub use state::{TransferState, TransferDirection, TransferError, TransferPriority};

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transfer {
    pub id: Uuid,
    pub file_name: String,
    pub file_size: u64,
    pub bytes_transferred: u64,
    #[serde(skip)]
    pub speed_tracker: MovingAverage,
    pub current_speed: u64,
    pub eta: Option<Duration>,
    pub state: TransferState,
    pub direction: TransferDirection,
    pub peer_id: Uuid,
    pub peer_name: String,
    #[serde(skip, default = "Instant::now")]
    pub started_at: Instant,
    #[serde(skip)]
    pub completed_at: Option<Instant>,
    pub error: Option<TransferError>,
    pub retry_count: u8,
    pub max_retries: u8,
    pub checksum: Option<String>,
    pub target_path: PathBuf,
    pub temp_path: PathBuf,
    pub priority: TransferPriority,
    pub auto_start: bool,
    pub verify_checksum: bool,
}

impl Transfer {
    pub fn new(
        file_name: String,
        file_size: u64,
        direction: TransferDirection,
        peer_id: Uuid,
        peer_name: String,
        target_path: PathBuf,
    ) -> Self {
        let id = Uuid::new_v4();
        let temp_path = target_path.with_extension(format!(".{}.tmp", id));

        Self {
            id,
            file_name,
            file_size,
            bytes_transferred: 0,
            speed_tracker: MovingAverage::new(10), // 10 sample moving average
            current_speed: 0,
            eta: None,
            state: TransferState::Pending,
            direction,
            peer_id,
            peer_name,
            started_at: Instant::now(),
            completed_at: None,
            error: None,
            retry_count: 0,
            max_retries: 3,
            checksum: None,
            target_path,
            temp_path,
            priority: TransferPriority::Normal,
            auto_start: true,
            verify_checksum: true,
        }
    }

    pub fn progress_percentage(&self) -> f32 {
        if self.file_size == 0 {
            return 0.0;
        }
        (self.bytes_transferred as f32 / self.file_size as f32) * 100.0
    }

    pub fn is_active(&self) -> bool {
        matches!(
            self.state,
            TransferState::Active | TransferState::Connecting | TransferState::Negotiating
        )
    }

    pub fn is_completed(&self) -> bool {
        matches!(self.state, TransferState::Completed)
    }

    pub fn is_failed(&self) -> bool {
        matches!(self.state, TransferState::Failed(_))
    }

    pub fn can_retry(&self) -> bool {
        self.retry_count < self.max_retries && !matches!(self.state, TransferState::Cancelled)
    }

    pub fn update_progress(&mut self, bytes: u64, speed: u64) {
        self.bytes_transferred = bytes;
        self.speed_tracker.add_sample(speed as f64);
        self.current_speed = self.speed_tracker.average() as u64;

        // Calculate ETA
        if self.current_speed > 0 {
            let remaining_bytes = self.file_size - self.bytes_transferred;
            let eta_seconds = remaining_bytes / self.current_speed;
            self.eta = Some(Duration::from_secs(eta_seconds));
        } else {
            self.eta = None;
        }
    }
}