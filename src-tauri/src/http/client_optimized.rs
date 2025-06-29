use bytes::Bytes;
use hyper::{Method, Request, Uri};
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::fs::OpenOptions;
use tokio::io::{AsyncWriteExt, AsyncSeekExt, BufWriter};
use tokio::sync::{Semaphore, RwLock};
use tracing::info;
use http_body_util::BodyExt;
use futures::future::join_all;

use crate::{FileshareError, Result};
use super::optimization::{FileCategory, TransferProfile, NetworkOptimizer, AdaptiveRateController};

pub struct OptimizedHttpClient {
    h1_client: Client<hyper_util::client::legacy::connect::HttpConnector, http_body_util::Empty<Bytes>>,
    h2_client: Client<hyper_util::client::legacy::connect::HttpConnector, http_body_util::Empty<Bytes>>,
    network_optimizer: Arc<RwLock<NetworkOptimizer>>,
    rate_controller: Arc<RwLock<AdaptiveRateController>>,
    active_connections: Arc<AtomicUsize>,
}

impl OptimizedHttpClient {
    pub fn new() -> Self {
        // HTTP/1.1 client optimized for throughput
        let h1_client = Client::builder(TokioExecutor::new())
            .pool_idle_timeout(std::time::Duration::from_secs(120))
            .pool_max_idle_per_host(64)
            .http2_only(false)
            .http1_max_buf_size(16 * 1024 * 1024) // 16MB max buffer
            .http1_title_case_headers(false)
            .http1_preserve_header_case(false)
            .build_http();
        
        // HTTP/2 client optimized for small files
        let h2_client = Client::builder(TokioExecutor::new())
            .pool_idle_timeout(std::time::Duration::from_secs(120))
            .pool_max_idle_per_host(32)
            .http2_only(true)
            .http2_initial_stream_window_size(1024 * 1024) // 1MB window
            .http2_initial_connection_window_size(16 * 1024 * 1024) // 16MB window
            .http2_max_concurrent_reset_streams(100)
            .build_http();
        
        Self { 
            h1_client,
            h2_client,
            network_optimizer: Arc::new(RwLock::new(NetworkOptimizer::new())),
            rate_controller: Arc::new(RwLock::new(AdaptiveRateController::new())),
            active_connections: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub async fn download_file_optimized(
        &self,
        url: String,
        target_path: PathBuf,
        expected_size: Option<u64>,
    ) -> Result<()> {
        // Get file size
        let file_size = if let Some(size) = expected_size {
            size
        } else {
            self.get_file_size(&url).await?
        };

        // Determine optimal profile based on file size
        let category = FileCategory::from_size(file_size);
        let profile = category.optimal_profile();
        
        info!("📊 File category: {:?}, Size: {:.2} MB", category, file_size as f64 / (1024.0 * 1024.0));
        info!("⚙️ Using profile: {} connections, {:.1}MB buffers, HTTP/{}", 
              profile.parallel_connections,
              profile.buffer_size as f64 / (1024.0 * 1024.0),
              if profile.use_http2 { "2" } else { "1.1" });

        // Use appropriate download strategy
        match category {
            FileCategory::Tiny | FileCategory::Small if profile.use_http2 => {
                self.download_http2_multiplexed(url, target_path, file_size, profile).await
            }
            _ => {
                self.download_parallel_adaptive(url, target_path, file_size, profile).await
            }
        }
    }

    async fn download_http2_multiplexed(
        &self,
        url: String,
        target_path: PathBuf,
        file_size: u64,
        profile: TransferProfile,
    ) -> Result<()> {
        let start_time = Instant::now();
        info!("⚡ Starting HTTP/2 multiplexed download");

        // Create target file
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&target_path)
            .await
            .map_err(|e| FileshareError::FileOperation(format!("Failed to create file: {}", e)))?;

        // Pre-allocate if supported
        self.preallocate_file(&mut file, file_size).await?;

        let bytes_downloaded = Arc::new(AtomicU64::new(0));
        let chunk_size = file_size / profile.parallel_connections as u64;

        // Create concurrent HTTP/2 streams
        let mut tasks = Vec::new();
        for i in 0..profile.parallel_connections {
            let start = i as u64 * chunk_size;
            let end = if i == profile.parallel_connections - 1 {
                file_size - 1
            } else {
                (i as u64 + 1) * chunk_size - 1
            };

            let url = url.clone();
            let target_path = target_path.clone();
            let client = self.h2_client.clone();
            let bytes_counter = bytes_downloaded.clone();
            let active_conns = self.active_connections.clone();

            let task = tokio::spawn(async move {
                active_conns.fetch_add(1, Ordering::Relaxed);
                let result = Self::download_range_h2(
                    client,
                    url,
                    target_path,
                    start,
                    end,
                    bytes_counter,
                    profile.buffer_size,
                    profile.write_threshold,
                ).await;
                active_conns.fetch_sub(1, Ordering::Relaxed);
                result
            });

            tasks.push(task);
        }

        // Monitor progress
        let monitor = self.start_progress_monitor(bytes_downloaded.clone(), file_size);

        // Wait for completion
        let results = join_all(tasks).await;
        monitor.abort();

        for result in results {
            match result {
                Ok(Ok(())) => {},
                Ok(Err(e)) => return Err(e),
                Err(e) => return Err(FileshareError::Transfer(format!("Task failed: {}", e))),
            }
        }

        file.sync_all().await?;

        let duration = start_time.elapsed();
        let throughput_mbps = (file_size as f64 * 8.0) / (duration.as_secs_f64() * 1_000_000.0);
        
        info!("✅ HTTP/2 download complete: {:.1} MB in {:.2}s ({:.1} Mbps)", 
              file_size as f64 / (1024.0 * 1024.0), 
              duration.as_secs_f64(), 
              throughput_mbps);

        // Update rate controller
        self.rate_controller.write().await.add_sample(throughput_mbps);

        Ok(())
    }

    async fn download_parallel_adaptive(
        &self,
        url: String,
        target_path: PathBuf,
        file_size: u64,
        mut profile: TransferProfile,
    ) -> Result<()> {
        let start_time = Instant::now();
        
        // Adjust connections based on network conditions
        let network_opt = self.network_optimizer.read().await;
        let optimal_connections = network_opt.calculate_optimal_connections(file_size);
        profile.parallel_connections = optimal_connections.min(profile.parallel_connections * 2);
        
        info!("⚡ Starting adaptive parallel download with {} connections", profile.parallel_connections);

        // Create and prepare file
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&target_path)
            .await?;

        self.preallocate_file(&mut file, file_size).await?;

        let bytes_downloaded = Arc::new(AtomicU64::new(0));
        let adaptive_connections = Arc::new(AtomicUsize::new(profile.parallel_connections));
        let semaphore = Arc::new(Semaphore::new(profile.parallel_connections * 2));

        // Start with initial connections
        let chunk_size = file_size / profile.parallel_connections as u64;
        let mut tasks = Vec::new();
        
        for i in 0..profile.parallel_connections {
            let start = i as u64 * chunk_size;
            let end = if i == profile.parallel_connections - 1 {
                file_size - 1
            } else {
                (i as u64 + 1) * chunk_size - 1
            };

            let task = self.spawn_download_task(
                url.clone(),
                target_path.clone(),
                start,
                end,
                bytes_downloaded.clone(),
                semaphore.clone(),
                profile,
            );
            tasks.push(task);
        }

        // Adaptive monitoring
        let monitor = self.start_adaptive_monitor(
            bytes_downloaded.clone(),
            file_size,
            adaptive_connections.clone(),
        );

        // Wait for completion
        let results = join_all(tasks).await;
        monitor.abort();

        for result in results {
            match result {
                Ok(Ok(())) => {},
                Ok(Err(e)) => return Err(e),
                Err(e) => return Err(FileshareError::Transfer(format!("Task failed: {}", e))),
            }
        }

        file.sync_all().await?;

        let duration = start_time.elapsed();
        let throughput_mbps = (file_size as f64 * 8.0) / (duration.as_secs_f64() * 1_000_000.0);
        
        info!("✅ Adaptive download complete: {:.1} MB in {:.2}s ({:.1} Mbps)", 
              file_size as f64 / (1024.0 * 1024.0), 
              duration.as_secs_f64(), 
              throughput_mbps);

        // Update controllers
        self.rate_controller.write().await.add_sample(throughput_mbps);

        Ok(())
    }

    async fn download_range_h2(
        client: Client<hyper_util::client::legacy::connect::HttpConnector, http_body_util::Empty<Bytes>>,
        url: String,
        target_path: PathBuf,
        start: u64,
        end: u64,
        bytes_counter: Arc<AtomicU64>,
        buffer_size: usize,
        write_threshold: usize,
    ) -> Result<()> {
        let uri: Uri = url.parse()
            .map_err(|e| FileshareError::Transfer(format!("Invalid URL: {}", e)))?;
        
        let req = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .header("User-Agent", "FileshareHTTP/2.0")
            .header("Range", format!("bytes={}-{}", start, end))
            .body(http_body_util::Empty::<Bytes>::new())
            .map_err(|e| FileshareError::Transfer(format!("Failed to build request: {}", e)))?;

        let res = client.request(req).await
            .map_err(|e| FileshareError::Transfer(format!("Request failed: {}", e)))?;

        if !res.status().is_success() && res.status().as_u16() != 206 {
            return Err(FileshareError::Transfer(format!("HTTP error: {}", res.status())));
        }

        // Open file with buffered writer
        let file = OpenOptions::new()
            .write(true)
            .open(&target_path)
            .await?;
        
        let mut writer = BufWriter::with_capacity(buffer_size, file);
        writer.seek(std::io::SeekFrom::Start(start)).await?;

        let mut body = res.into_body();
        let mut buffer = Vec::with_capacity(buffer_size);

        while let Some(chunk) = body.frame().await {
            let chunk = chunk
                .map_err(|e| FileshareError::Transfer(format!("Failed to read chunk: {}", e)))?;
            
            if let Some(data) = chunk.data_ref() {
                buffer.extend_from_slice(data);
                
                if buffer.len() >= write_threshold {
                    writer.write_all(&buffer).await?;
                    bytes_counter.fetch_add(buffer.len() as u64, Ordering::Relaxed);
                    buffer.clear();
                }
            }
        }

        if !buffer.is_empty() {
            writer.write_all(&buffer).await?;
            bytes_counter.fetch_add(buffer.len() as u64, Ordering::Relaxed);
        }

        writer.flush().await?;
        Ok(())
    }

    fn spawn_download_task(
        &self,
        url: String,
        target_path: PathBuf,
        start: u64,
        end: u64,
        bytes_counter: Arc<AtomicU64>,
        semaphore: Arc<Semaphore>,
        profile: TransferProfile,
    ) -> tokio::task::JoinHandle<Result<()>> {
        let client = self.h1_client.clone();
        let active_conns = self.active_connections.clone();

        tokio::spawn(async move {
            let _permit = semaphore.acquire().await
                .map_err(|_| FileshareError::Transfer("Failed to acquire semaphore".to_string()))?;
            active_conns.fetch_add(1, Ordering::Relaxed);
            
            let result = Self::download_range_optimized(
                client,
                url,
                target_path,
                start,
                end,
                bytes_counter,
                profile,
            ).await;
            
            active_conns.fetch_sub(1, Ordering::Relaxed);
            result
        })
    }

    async fn download_range_optimized(
        client: Client<hyper_util::client::legacy::connect::HttpConnector, http_body_util::Empty<Bytes>>,
        url: String,
        target_path: PathBuf,
        start: u64,
        end: u64,
        bytes_counter: Arc<AtomicU64>,
        profile: TransferProfile,
    ) -> Result<()> {
        let uri: Uri = url.parse()
            .map_err(|e| FileshareError::Transfer(format!("Invalid URL: {}", e)))?;
        
        let req = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .header("User-Agent", "FileshareHTTP/1.1-Optimized")
            .header("Range", format!("bytes={}-{}", start, end))
            .header("Connection", "keep-alive")
            .body(http_body_util::Empty::<Bytes>::new())
            .map_err(|e| FileshareError::Transfer(format!("Failed to build request: {}", e)))?;

        let res = client.request(req).await
            .map_err(|e| FileshareError::Transfer(format!("Request failed: {}", e)))?;

        if !res.status().is_success() && res.status().as_u16() != 206 {
            return Err(FileshareError::Transfer(format!("HTTP error: {}", res.status())));
        }

        // Use direct I/O for large files
        let file = OpenOptions::new()
            .write(true)
            .open(&target_path)
            .await?;

        // Use buffered writer with profile-specific buffer
        let mut writer = BufWriter::with_capacity(profile.buffer_size, file);
        writer.seek(std::io::SeekFrom::Start(start)).await?;

        let mut body = res.into_body();
        let mut buffer = Vec::with_capacity(profile.buffer_size);
        let mut prefetch_buffer = Vec::with_capacity(profile.prefetch_size);

        while let Some(chunk) = body.frame().await {
            let chunk = chunk
                .map_err(|e| FileshareError::Transfer(format!("Failed to read chunk: {}", e)))?;
            
            if let Some(data) = chunk.data_ref() {
                // Fill prefetch buffer first
                if prefetch_buffer.len() < profile.prefetch_size {
                    prefetch_buffer.extend_from_slice(data);
                    continue;
                }

                // Process prefetch buffer
                buffer.extend_from_slice(&prefetch_buffer);
                prefetch_buffer.clear();
                prefetch_buffer.extend_from_slice(data);
                
                if buffer.len() >= profile.write_threshold {
                    writer.write_all(&buffer).await?;
                    bytes_counter.fetch_add(buffer.len() as u64, Ordering::Relaxed);
                    buffer.clear();
                }
            }
        }

        // Write remaining data
        if !prefetch_buffer.is_empty() {
            buffer.extend_from_slice(&prefetch_buffer);
        }
        if !buffer.is_empty() {
            writer.write_all(&buffer).await?;
            bytes_counter.fetch_add(buffer.len() as u64, Ordering::Relaxed);
        }

        writer.flush().await?;
        Ok(())
    }

    async fn preallocate_file(&self, file: &mut tokio::fs::File, size: u64) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::io::AsRawFd;
            let fd = file.as_raw_fd();
            unsafe {
                let ret = libc::fallocate(fd, 0, 0, size as libc::off_t);
                if ret != 0 {
                    file.set_len(size).await?;
                }
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            file.set_len(size).await?;
        }
        file.seek(std::io::SeekFrom::Start(0)).await?;
        Ok(())
    }

    fn start_progress_monitor(
        &self,
        bytes_counter: Arc<AtomicU64>,
        file_size: u64,
    ) -> tokio::task::JoinHandle<()> {
        let active_conns = self.active_connections.clone();
        
        tokio::spawn(async move {
            let mut last_bytes = 0u64;
            let mut last_time = Instant::now();
            
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                let current_bytes = bytes_counter.load(Ordering::Relaxed);
                let current_time = Instant::now();
                let elapsed = current_time.duration_since(last_time).as_secs_f64();
                let active = active_conns.load(Ordering::Relaxed);
                
                if elapsed > 0.0 && file_size > 0 {
                    let speed_mbps = ((current_bytes - last_bytes) as f64 * 8.0) / (elapsed * 1_000_000.0);
                    let progress = (current_bytes as f64 / file_size as f64) * 100.0;
                    
                    if current_bytes < file_size {
                        info!("⬇️ Progress: {:.1}% - Speed: {:.1} Mbps - Active connections: {}", 
                              progress, speed_mbps, active);
                    }
                }
                
                last_bytes = current_bytes;
                last_time = current_time;
                
                if current_bytes >= file_size {
                    break;
                }
            }
        })
    }

    fn start_adaptive_monitor(
        &self,
        bytes_counter: Arc<AtomicU64>,
        file_size: u64,
        adaptive_connections: Arc<AtomicUsize>,
    ) -> tokio::task::JoinHandle<()> {
        let rate_controller = self.rate_controller.clone();
        let active_conns = self.active_connections.clone();
        
        tokio::spawn(async move {
            let mut last_bytes = 0u64;
            let mut last_time = Instant::now();
            let mut sample_count = 0;
            
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                let current_bytes = bytes_counter.load(Ordering::Relaxed);
                let current_time = Instant::now();
                let elapsed = current_time.duration_since(last_time).as_secs_f64();
                let active = active_conns.load(Ordering::Relaxed);
                
                if elapsed > 0.0 && file_size > 0 {
                    let speed_mbps = ((current_bytes - last_bytes) as f64 * 8.0) / (elapsed * 1_000_000.0);
                    let progress = (current_bytes as f64 / file_size as f64) * 100.0;
                    
                    if current_bytes < file_size {
                        info!("⬇️ Progress: {:.1}% - Speed: {:.1} Mbps - Active: {} connections", 
                              progress, speed_mbps, active);
                        
                        // Adaptive adjustment every 3 samples
                        sample_count += 1;
                        if sample_count % 3 == 0 {
                            let mut controller = rate_controller.write().await;
                            controller.add_sample(speed_mbps);
                            
                            let current_conns = adaptive_connections.load(Ordering::Relaxed);
                            if controller.should_increase_connections(current_conns) {
                                adaptive_connections.store(current_conns + 2, Ordering::Relaxed);
                                info!("📈 Increasing connections to {}", current_conns + 2);
                            } else if controller.should_decrease_connections(current_conns) {
                                adaptive_connections.store(current_conns.saturating_sub(1), Ordering::Relaxed);
                                info!("📉 Decreasing connections to {}", current_conns.saturating_sub(1));
                            }
                        }
                    }
                }
                
                last_bytes = current_bytes;
                last_time = current_time;
                
                if current_bytes >= file_size {
                    break;
                }
            }
        })
    }

    async fn get_file_size(&self, url: &str) -> Result<u64> {
        let uri: Uri = url.parse()
            .map_err(|e| FileshareError::Transfer(format!("Invalid URL: {}", e)))?;

        let req = Request::builder()
            .method(Method::HEAD)
            .uri(uri)
            .header("User-Agent", "FileshareHTTP/2.0")
            .body(http_body_util::Empty::<Bytes>::new())
            .map_err(|e| FileshareError::Transfer(format!("Failed to build request: {}", e)))?;

        let res = self.h1_client.request(req).await
            .map_err(|e| FileshareError::Transfer(format!("Request failed: {}", e)))?;

        let content_length = res.headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .ok_or_else(|| FileshareError::Transfer("No content-length header".to_string()))?;

        Ok(content_length)
    }
}

impl Clone for OptimizedHttpClient {
    fn clone(&self) -> Self {
        Self::new()
    }
}

#[cfg(target_os = "linux")]
extern crate libc;