use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub struct SpeedCalculator {
    samples: VecDeque<(Instant, u64)>,
    max_samples: usize,
    window_duration: Duration,
}

impl SpeedCalculator {
    pub fn new() -> Self {
        Self {
            samples: VecDeque::with_capacity(20),
            max_samples: 20,
            window_duration: Duration::from_secs(3),
        }
    }

    pub fn add_sample(&mut self, bytes: u64) {
        let now = Instant::now();

        self.samples.retain(|(time, _)| {
            now.duration_since(*time) < self.window_duration
        });

        self.samples.push_back((now, bytes));

        if self.samples.len() > self.max_samples {
            self.samples.pop_front();
        }
    }

    pub fn calculate_speed_bps(&self) -> f64 {
        if self.samples.len() < 2 {
            return 0.0;
        }

        let (first_time, first_bytes) = self.samples.front().unwrap();
        let (last_time, last_bytes) = self.samples.back().unwrap();

        let duration = last_time.duration_since(*first_time);
        if duration.as_secs_f64() == 0.0 {
            return 0.0;
        }

        let bytes_diff = last_bytes.saturating_sub(*first_bytes);
        bytes_diff as f64 / duration.as_secs_f64()
    }

    pub fn calculate_eta(&self, remaining_bytes: u64) -> Option<u32> {
        let speed_bps = self.calculate_speed_bps();
        if speed_bps <= 0.0 {
            return None;
        }

        let eta_seconds = remaining_bytes as f64 / speed_bps;
        if eta_seconds.is_finite() && eta_seconds > 0.0 {
            Some(eta_seconds as u32)
        } else {
            None
        }
    }
}

pub fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit_index = 0;

    while size >= 1024.0 && unit_index < UNITS.len() - 1 {
        size /= 1024.0;
        unit_index += 1;
    }

    format!("{:.2} {}", size, UNITS[unit_index])
}

pub fn format_speed(bytes_per_second: f64) -> String {
    let mbps = bytes_per_second * 8.0 / (1024.0 * 1024.0);
    format!("{:.1} Mbps", mbps)
}

pub fn format_eta(seconds: u32) -> String {
    if seconds < 60 {
        format!("{}s", seconds)
    } else if seconds < 3600 {
        let minutes = seconds / 60;
        let remaining_seconds = seconds % 60;
        if remaining_seconds > 0 {
            format!("{}m {}s", minutes, remaining_seconds)
        } else {
            format!("{}m", minutes)
        }
    } else {
        let hours = seconds / 3600;
        let remaining_minutes = (seconds % 3600) / 60;
        if remaining_minutes > 0 {
            format!("{}h {}m", hours, remaining_minutes)
        } else {
            format!("{}h", hours)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(512), "512.00 B");
        assert_eq!(format_bytes(1024), "1.00 KB");
        assert_eq!(format_bytes(1024 * 1024), "1.00 MB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.00 GB");
    }

    #[test]
    fn test_format_speed() {
        assert_eq!(format_speed(131072.0), "1.0 Mbps");
        assert_eq!(format_speed(1310720.0), "10.0 Mbps");
    }

    #[test]
    fn test_format_eta() {
        assert_eq!(format_eta(30), "30s");
        assert_eq!(format_eta(90), "1m 30s");
        assert_eq!(format_eta(3600), "1h");
        assert_eq!(format_eta(3720), "1h 2m");
    }
}