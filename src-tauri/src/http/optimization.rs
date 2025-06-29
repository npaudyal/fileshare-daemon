
#[derive(Debug, Clone, Copy)]
pub struct TransferProfile {
    pub parallel_connections: usize,
    pub buffer_size: usize,
    pub write_threshold: usize,
    pub tcp_buffer_size: u32,
    pub use_http2: bool,
    pub prefetch_size: usize,
    pub io_threads: usize,
}

#[derive(Debug, Clone, Copy)]
pub enum FileCategory {
    Tiny,      // < 1MB
    Small,     // 1MB - 10MB  
    Medium,    // 10MB - 100MB
    Large,     // 100MB - 1GB
    Huge,      // > 1GB
}

impl FileCategory {
    pub fn from_size(size: u64) -> Self {
        match size {
            0..=1_048_576 => FileCategory::Tiny,
            1_048_577..=10_485_760 => FileCategory::Small,
            10_485_761..=104_857_600 => FileCategory::Medium,
            104_857_601..=1_073_741_824 => FileCategory::Large,
            _ => FileCategory::Huge,
        }
    }

    pub fn optimal_profile(&self) -> TransferProfile {
        match self {
            FileCategory::Tiny => TransferProfile {
                parallel_connections: 1,
                buffer_size: 64 * 1024,           // 64KB
                write_threshold: 32 * 1024,       // 32KB
                tcp_buffer_size: 256 * 1024,      // 256KB
                use_http2: true,                  // HTTP/2 for small files
                prefetch_size: 64 * 1024,         // 64KB prefetch
                io_threads: 1,
            },
            FileCategory::Small => TransferProfile {
                parallel_connections: 2,
                buffer_size: 256 * 1024,          // 256KB
                write_threshold: 128 * 1024,      // 128KB
                tcp_buffer_size: 1024 * 1024,     // 1MB
                use_http2: true,                  // HTTP/2 for multiplexing
                prefetch_size: 256 * 1024,        // 256KB prefetch
                io_threads: 2,
            },
            FileCategory::Medium => TransferProfile {
                parallel_connections: 4,
                buffer_size: 2 * 1024 * 1024,     // 2MB
                write_threshold: 1024 * 1024,     // 1MB
                tcp_buffer_size: 4 * 1024 * 1024, // 4MB
                use_http2: false,                 // HTTP/1.1 for better throughput
                prefetch_size: 2 * 1024 * 1024,   // 2MB prefetch
                io_threads: 4,
            },
            FileCategory::Large => TransferProfile {
                parallel_connections: 8,
                buffer_size: 8 * 1024 * 1024,     // 8MB
                write_threshold: 4 * 1024 * 1024, // 4MB
                tcp_buffer_size: 16 * 1024 * 1024,// 16MB
                use_http2: false,                 // HTTP/1.1
                prefetch_size: 8 * 1024 * 1024,   // 8MB prefetch
                io_threads: 6,
            },
            FileCategory::Huge => TransferProfile {
                parallel_connections: 16,
                buffer_size: 16 * 1024 * 1024,    // 16MB
                write_threshold: 8 * 1024 * 1024, // 8MB
                tcp_buffer_size: 32 * 1024 * 1024,// 32MB
                use_http2: false,                 // HTTP/1.1
                prefetch_size: 16 * 1024 * 1024,  // 16MB prefetch
                io_threads: 8,
            },
        }
    }
}

pub struct NetworkOptimizer {
    pub rtt_ms: f64,
    pub bandwidth_mbps: f64,
}

impl NetworkOptimizer {
    pub fn new() -> Self {
        Self {
            rtt_ms: 1.0,           // LAN RTT estimate
            bandwidth_mbps: 1000.0, // Gigabit default
        }
    }

    pub fn calculate_optimal_window(&self) -> u32 {
        // BDP = bandwidth * RTT
        let bdp_bytes = (self.bandwidth_mbps * 1_000_000.0 / 8.0) * (self.rtt_ms / 1000.0);
        // Use 2x BDP for optimal performance
        (bdp_bytes * 2.0).min(64.0 * 1024.0 * 1024.0) as u32
    }

    pub fn calculate_optimal_connections(&self, file_size: u64) -> usize {
        let base_connections = FileCategory::from_size(file_size).optimal_profile().parallel_connections;
        
        // Adjust based on bandwidth
        if self.bandwidth_mbps >= 10000.0 {
            // 10Gbps+ network
            base_connections * 2
        } else if self.bandwidth_mbps >= 2500.0 {
            // 2.5Gbps+ network  
            (base_connections as f64 * 1.5) as usize
        } else {
            base_connections
        }
    }
}

#[cfg(target_os = "linux")]
pub fn apply_tcp_optimizations(fd: i32, profile: &TransferProfile) {
    use libc::{setsockopt, SOL_SOCKET, SO_SNDBUF, SO_RCVBUF, IPPROTO_TCP, TCP_NODELAY};
    
    unsafe {
        // Set send/receive buffer sizes
        let buf_size = profile.tcp_buffer_size as libc::c_int;
        setsockopt(
            fd,
            SOL_SOCKET,
            SO_SNDBUF,
            &buf_size as *const _ as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        );
        
        setsockopt(
            fd,
            SOL_SOCKET,
            SO_RCVBUF,
            &buf_size as *const _ as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        );
        
        // Enable TCP_NODELAY for low latency
        let nodelay: libc::c_int = 1;
        setsockopt(
            fd,
            IPPROTO_TCP,
            TCP_NODELAY,
            &nodelay as *const _ as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        );
    }
}

pub struct AdaptiveRateController {
    samples: Vec<f64>,
    window_size: usize,
}

impl AdaptiveRateController {
    pub fn new() -> Self {
        Self {
            samples: Vec::with_capacity(10),
            window_size: 5,
        }
    }

    pub fn add_sample(&mut self, mbps: f64) {
        self.samples.push(mbps);
        if self.samples.len() > self.window_size {
            self.samples.remove(0);
        }
    }

    pub fn get_average_rate(&self) -> f64 {
        if self.samples.is_empty() {
            0.0
        } else {
            self.samples.iter().sum::<f64>() / self.samples.len() as f64
        }
    }

    pub fn should_increase_connections(&self, current_connections: usize) -> bool {
        if self.samples.len() < 3 {
            return false;
        }
        
        // Check if rate is increasing
        let recent_avg = self.samples.iter().rev().take(2).sum::<f64>() / 2.0;
        let older_avg = self.samples.iter().take(2).sum::<f64>() / 2.0;
        
        recent_avg > older_avg * 1.1 && current_connections < 32
    }

    pub fn should_decrease_connections(&self, current_connections: usize) -> bool {
        if self.samples.len() < 3 || current_connections <= 1 {
            return false;
        }
        
        // Check if rate is decreasing
        let recent_avg = self.samples.iter().rev().take(2).sum::<f64>() / 2.0;
        let older_avg = self.samples.iter().take(2).sum::<f64>() / 2.0;
        
        recent_avg < older_avg * 0.9
    }
}