use bytes::Bytes;
use hyper::{Method, Request, Uri};
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::fs::OpenOptions;
use tokio::io::{AsyncWriteExt, AsyncSeekExt};
use tokio::sync::Semaphore;
use tracing::{info, debug};
use http_body_util::BodyExt;
use futures::future::join_all;

use crate::{FileshareError, Result};

// Optimized buffer sizes for LAN transfers
const DOWNLOAD_BUFFER_SIZE: usize = 8 * 1024 * 1024; // 8MB buffer for optimal performance
const WRITE_THRESHOLD: usize = 2 * 1024 * 1024; // Write every 2MB for continuous streaming
const PARALLEL_CONNECTIONS: usize = 4; // Number of parallel connections for large files
const LARGE_FILE_THRESHOLD: u64 = 50 * 1024 * 1024; // Files > 50MB use parallel downloads

/// Progress callback type - returns (bytes_downloaded, total_bytes, bytes_per_second)
pub type ProgressCallback = Box<dyn Fn(u64, u64, u64) + Send + Sync>;

pub struct HttpFileClient {
    client: Client<hyper_util::client::legacy::connect::HttpConnector, http_body_util::Empty<Bytes>>,
    parallel_semaphore: Arc<Semaphore>,
}

impl HttpFileClient {
    pub fn new() -> Self {
        let client = Client::builder(TokioExecutor::new())
            .pool_idle_timeout(std::time::Duration::from_secs(60))
            .pool_max_idle_per_host(32)
            .http2_only(false) // Use HTTP/1.1 for better compatibility
            .http1_max_buf_size(DOWNLOAD_BUFFER_SIZE)
            .http1_title_case_headers(false)
            .http1_preserve_header_case(false)
            .build_http();
        
        Self { 
            client,
            parallel_semaphore: Arc::new(Semaphore::new(PARALLEL_CONNECTIONS)),
        }
    }

    /// Download a file from the given URL with progress callback
    pub async fn download_file_with_progress(
        &self,
        url: String,
        target_path: PathBuf,
        expected_size: Option<u64>,
        progress_callback: Option<ProgressCallback>,
    ) -> Result<()> {
        // First, get file size if not provided
        let file_size = if let Some(size) = expected_size {
            size
        } else {
            self.get_file_size(&url).await?
        };

        // Decide whether to use parallel downloads based on file size
        if file_size > LARGE_FILE_THRESHOLD {
            info!("📊 Large file detected ({:.1} MB), using parallel downloads",
                  file_size as f64 / (1024.0 * 1024.0));
            self.download_file_parallel_with_progress(url, target_path, file_size, progress_callback).await
        } else {
            self.download_file_single_with_progress(url, target_path, Some(file_size), progress_callback).await
        }
    }

    /// Download a file from the given URL with optimizations (legacy method without progress)
    pub async fn download_file(
        &self,
        url: String,
        target_path: PathBuf,
        expected_size: Option<u64>,
    ) -> Result<()> {
        // First, get file size if not provided
        let file_size = if let Some(size) = expected_size {
            size
        } else {
            self.get_file_size(&url).await?
        };

        // Decide whether to use parallel downloads based on file size
        if file_size > LARGE_FILE_THRESHOLD {
            info!("📊 Large file detected ({:.1} MB), using parallel downloads", 
                  file_size as f64 / (1024.0 * 1024.0));
            self.download_file_parallel(url, target_path, file_size).await
        } else {
            self.download_file_single(url, target_path, Some(file_size)).await
        }
    }

    /// Get file size from HTTP HEAD request
    async fn get_file_size(&self, url: &str) -> Result<u64> {
        let uri: Uri = url.parse()
            .map_err(|e| FileshareError::Transfer(format!("Invalid URL: {}", e)))?;

        let req = Request::builder()
            .method(Method::HEAD)
            .uri(uri)
            .header("User-Agent", "FileshareHTTP/1.0")
            .body(http_body_util::Empty::<Bytes>::new())
            .map_err(|e| FileshareError::Transfer(format!("Failed to build request: {}", e)))?;

        let res = self.client.request(req).await
            .map_err(|e| FileshareError::Transfer(format!("HEAD request failed: {}", e)))?;

        let content_length = res.headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .ok_or_else(|| FileshareError::Transfer("No content-length header".to_string()))?;

        Ok(content_length)
    }

    /// Download file using parallel connections with HTTP range requests
    async fn download_file_parallel(
        &self,
        url: String,
        target_path: PathBuf,
        file_size: u64,
    ) -> Result<()> {
        let start_time = Instant::now();
        info!("⚡ Starting parallel HTTP download with {} connections", PARALLEL_CONNECTIONS);

        // Create target file and pre-allocate space
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&target_path)
            .await
            .map_err(|e| FileshareError::FileOperation(format!("Failed to create file: {}", e)))?;

        // Pre-allocate file space for better performance
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::io::AsRawFd;
            let fd = file.as_raw_fd();
            unsafe {
                let ret = libc::fallocate(fd, 0, 0, file_size as libc::off_t);
                if ret != 0 {
                    debug!("fallocate not supported, using set_len");
                    file.set_len(file_size).await?;
                }
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            file.set_len(file_size).await?;
        }

        // Calculate chunk size for each parallel download
        let chunk_size = file_size / PARALLEL_CONNECTIONS as u64;
        let bytes_downloaded = Arc::new(AtomicU64::new(0));
        let download_complete = Arc::new(AtomicBool::new(false));

        // Start progress monitor
        let bytes_downloaded_monitor = bytes_downloaded.clone();
        let download_complete_monitor = download_complete.clone();
        let monitor_handle = tokio::spawn(async move {
            let mut last_bytes = 0u64;
            let mut last_time = Instant::now();
            
            while !download_complete_monitor.load(Ordering::Relaxed) {
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                let current_bytes = bytes_downloaded_monitor.load(Ordering::Relaxed);
                let current_time = Instant::now();
                let elapsed = current_time.duration_since(last_time).as_secs_f64();
                
                if elapsed > 0.0 && file_size > 0 {
                    let speed_mbps = ((current_bytes - last_bytes) as f64 * 8.0) / (elapsed * 1_000_000.0);
                    let progress = (current_bytes as f64 / file_size as f64) * 100.0;
                    
                    info!("⬇️ Progress: {:.1}% - Speed: {:.1} Mbps", progress, speed_mbps);
                }
                
                last_bytes = current_bytes;
                last_time = current_time;
            }
        });

        // Create parallel download tasks
        let mut tasks = Vec::new();
        for i in 0..PARALLEL_CONNECTIONS {
            let start = i as u64 * chunk_size;
            let end = if i == PARALLEL_CONNECTIONS - 1 {
                file_size - 1
            } else {
                (i as u64 + 1) * chunk_size - 1
            };

            let url_clone = url.clone();
            let target_path_clone = target_path.clone();
            let client = self.clone();
            let bytes_downloaded_clone = bytes_downloaded.clone();
            let semaphore = self.parallel_semaphore.clone();

            let task = tokio::spawn(async move {
                let _permit = semaphore.acquire().await
                    .map_err(|_| FileshareError::Transfer("Failed to acquire semaphore".to_string()))?;
                client.download_range(
                    url_clone,
                    target_path_clone,
                    start,
                    end,
                    bytes_downloaded_clone,
                ).await
            });

            tasks.push(task);
        }

        // Wait for all downloads to complete
        let results = join_all(tasks).await;
        download_complete.store(true, Ordering::Relaxed);
        monitor_handle.abort();

        // Check for errors
        for result in results {
            match result {
                Ok(Ok(())) => {},
                Ok(Err(e)) => return Err(e),
                Err(e) => return Err(FileshareError::Transfer(format!("Task failed: {}", e))),
            }
        }

        // Ensure all data is written
        file.sync_all().await
            .map_err(|e| FileshareError::FileOperation(format!("Failed to sync: {}", e)))?;

        let duration = start_time.elapsed();
        let throughput_mbps = (file_size as f64 * 8.0) / (duration.as_secs_f64() * 1_000_000.0);
        
        info!("✅ Parallel HTTP download complete: {:.1} MB in {:.2}s ({:.1} Mbps)", 
              file_size as f64 / (1024.0 * 1024.0), 
              duration.as_secs_f64(), 
              throughput_mbps);

        Ok(())
    }

    /// Download a specific range of bytes
    async fn download_range(
        &self,
        url: String,
        target_path: PathBuf,
        start: u64,
        end: u64,
        bytes_counter: Arc<AtomicU64>,
    ) -> Result<()> {
        debug!("Downloading range {}-{}", start, end);

        let uri: Uri = url.parse()
            .map_err(|e| FileshareError::Transfer(format!("Invalid URL: {}", e)))?;

        // Create HTTP request with Range header
        let req = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .header("User-Agent", "FileshareHTTP/1.0")
            .header("Range", format!("bytes={}-{}", start, end))
            .body(http_body_util::Empty::<Bytes>::new())
            .map_err(|e| FileshareError::Transfer(format!("Failed to build request: {}", e)))?;

        let res = self.client.request(req).await
            .map_err(|e| FileshareError::Transfer(format!("Request failed: {}", e)))?;

        if !res.status().is_success() && res.status().as_u16() != 206 {
            return Err(FileshareError::Transfer(format!("HTTP error: {}", res.status())));
        }

        // Open file for writing at specific offset
        let mut file = OpenOptions::new()
            .write(true)
            .open(&target_path)
            .await
            .map_err(|e| FileshareError::FileOperation(format!("Failed to open file: {}", e)))?;

        file.seek(std::io::SeekFrom::Start(start)).await
            .map_err(|e| FileshareError::FileOperation(format!("Failed to seek: {}", e)))?;

        // Stream body to file with optimized buffering
        let mut body = res.into_body();
        let mut buffer = Vec::with_capacity(DOWNLOAD_BUFFER_SIZE);

        while let Some(chunk) = body.frame().await {
            let chunk = chunk.map_err(|e| FileshareError::Transfer(format!("Failed to read chunk: {}", e)))?;
            
            if let Some(data) = chunk.data_ref() {
                buffer.extend_from_slice(data);
                
                // Write when buffer reaches threshold
                if buffer.len() >= WRITE_THRESHOLD {
                    file.write_all(&buffer).await
                        .map_err(|e| FileshareError::FileOperation(format!("Failed to write: {}", e)))?;
                    
                    let written = buffer.len() as u64;
                    bytes_counter.fetch_add(written, Ordering::Relaxed);
                    buffer.clear();
                }
            }
        }

        // Write remaining data
        if !buffer.is_empty() {
            file.write_all(&buffer).await
                .map_err(|e| FileshareError::FileOperation(format!("Failed to write final chunk: {}", e)))?;
            bytes_counter.fetch_add(buffer.len() as u64, Ordering::Relaxed);
        }

        // Ensure data is written for this range
        file.flush().await
            .map_err(|e| FileshareError::FileOperation(format!("Failed to flush: {}", e)))?;

        Ok(())
    }

    /// Download file using single connection (for smaller files)
    async fn download_file_single(
        &self,
        url: String,
        target_path: PathBuf,
        expected_size: Option<u64>,
    ) -> Result<()> {
        let start_time = Instant::now();
        info!("⬇️ Starting HTTP download from: {}", url);

        let uri: Uri = url.parse()
            .map_err(|e| FileshareError::Transfer(format!("Invalid URL: {}", e)))?;

        let req = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .header("User-Agent", "FileshareHTTP/1.0")
            .body(http_body_util::Empty::<Bytes>::new())
            .map_err(|e| FileshareError::Transfer(format!("Failed to build request: {}", e)))?;

        let res = self.client.request(req).await
            .map_err(|e| FileshareError::Transfer(format!("Request failed: {}", e)))?;

        if !res.status().is_success() {
            return Err(FileshareError::Transfer(format!("HTTP error: {}", res.status())));
        }

        let content_length = res.headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());

        let file_size = content_length.or(expected_size).unwrap_or(0);
        info!("📊 Downloading {:.1} MB", file_size as f64 / (1024.0 * 1024.0));

        // Create target file
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&target_path)
            .await
            .map_err(|e| FileshareError::FileOperation(format!("Failed to create file: {}", e)))?;

        // Pre-allocate file if size is known
        if file_size > 0 {
            #[cfg(target_os = "linux")]
            {
                use std::os::unix::io::AsRawFd;
                let fd = file.as_raw_fd();
                unsafe {
                    let ret = libc::fallocate(fd, 0, 0, file_size as libc::off_t);
                    if ret != 0 {
                        file.set_len(file_size).await?;
                    }
                }
            }
            #[cfg(not(target_os = "linux"))]
            {
                file.set_len(file_size).await?;
            }
            file.seek(std::io::SeekFrom::Start(0)).await?;
        }

        // Download with optimized buffering
        let bytes_downloaded = Arc::new(AtomicU64::new(0));
        let bytes_downloaded_monitor = bytes_downloaded.clone();

        // Start progress monitor
        let monitor_handle = tokio::spawn(async move {
            let mut last_bytes = 0u64;
            let mut last_time = Instant::now();
            
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                let current_bytes = bytes_downloaded_monitor.load(Ordering::Relaxed);
                let current_time = Instant::now();
                let elapsed = current_time.duration_since(last_time).as_secs_f64();
                
                if elapsed > 0.0 && file_size > 0 {
                    let speed_mbps = ((current_bytes - last_bytes) as f64 * 8.0) / (elapsed * 1_000_000.0);
                    let progress = (current_bytes as f64 / file_size as f64) * 100.0;
                    
                    if current_bytes < file_size {
                        info!("⬇️ Progress: {:.1}% - Speed: {:.1} Mbps", progress, speed_mbps);
                    }
                }
                
                last_bytes = current_bytes;
                last_time = current_time;
                
                if current_bytes >= file_size && file_size > 0 {
                    break;
                }
            }
        });

        // Stream body to file with continuous writes
        let mut body = res.into_body();
        let mut buffer = Vec::with_capacity(DOWNLOAD_BUFFER_SIZE);
        let mut total_written = 0u64;

        while let Some(chunk) = body.frame().await {
            let chunk = chunk.map_err(|e| FileshareError::Transfer(format!("Failed to read chunk: {}", e)))?;
            
            if let Some(data) = chunk.data_ref() {
                buffer.extend_from_slice(data);
                
                // Write more frequently for continuous streaming
                if buffer.len() >= WRITE_THRESHOLD {
                    file.write_all(&buffer).await
                        .map_err(|e| FileshareError::FileOperation(format!("Failed to write: {}", e)))?;
                    
                    total_written += buffer.len() as u64;
                    bytes_downloaded.store(total_written, Ordering::Relaxed);
                    buffer.clear();
                }
            }
        }

        // Write any remaining data
        if !buffer.is_empty() {
            file.write_all(&buffer).await
                .map_err(|e| FileshareError::FileOperation(format!("Failed to write final chunk: {}", e)))?;
            total_written += buffer.len() as u64;
        }

        // Ensure all data is written
        file.flush().await
            .map_err(|e| FileshareError::FileOperation(format!("Failed to flush: {}", e)))?;
        file.sync_all().await
            .map_err(|e| FileshareError::FileOperation(format!("Failed to sync: {}", e)))?;

        monitor_handle.abort();

        let duration = start_time.elapsed();
        let throughput_mbps = (total_written as f64 * 8.0) / (duration.as_secs_f64() * 1_000_000.0);
        
        info!("✅ HTTP download complete: {:.1} MB in {:.2}s ({:.1} Mbps)", 
              total_written as f64 / (1024.0 * 1024.0), 
              duration.as_secs_f64(), 
              throughput_mbps);

        Ok(())
    }

    /// Download file using parallel connections with progress callback
    async fn download_file_parallel_with_progress(
        &self,
        url: String,
        target_path: PathBuf,
        file_size: u64,
        progress_callback: Option<ProgressCallback>,
    ) -> Result<()> {
        let start_time = Instant::now();
        info!("⚡ Starting parallel HTTP download with {} connections", PARALLEL_CONNECTIONS);

        // Create target file and pre-allocate space
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&target_path)
            .await
            .map_err(|e| FileshareError::FileOperation(format!("Failed to create file: {}", e)))?;

        // Pre-allocate file space for better performance
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::io::AsRawFd;
            let fd = file.as_raw_fd();
            unsafe {
                let ret = libc::fallocate(fd, 0, 0, file_size as libc::off_t);
                if ret != 0 {
                    debug!("fallocate not supported, using set_len");
                    file.set_len(file_size).await?;
                }
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            file.set_len(file_size).await?;
        }

        // Calculate chunk size for each parallel download
        let chunk_size = file_size / PARALLEL_CONNECTIONS as u64;
        let bytes_downloaded = Arc::new(AtomicU64::new(0));
        let download_complete = Arc::new(AtomicBool::new(false));
        let progress_callback = Arc::new(progress_callback);

        // Start progress monitor with callback
        let bytes_downloaded_monitor = bytes_downloaded.clone();
        let download_complete_monitor = download_complete.clone();
        let progress_callback_monitor = progress_callback.clone();
        let monitor_handle = tokio::spawn(async move {
            let mut last_bytes = 0u64;
            let mut last_time = Instant::now();

            while !download_complete_monitor.load(Ordering::Relaxed) {
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                let current_bytes = bytes_downloaded_monitor.load(Ordering::Relaxed);
                let current_time = Instant::now();
                let elapsed = current_time.duration_since(last_time).as_secs_f64();

                if elapsed > 0.0 {
                    let speed = ((current_bytes - last_bytes) as f64 / elapsed) as u64;

                    // Call progress callback if provided
                    if let Some(ref callback) = *progress_callback_monitor {
                        callback(current_bytes, file_size, speed);
                    }

                    last_bytes = current_bytes;
                    last_time = current_time;
                }
            }
        });

        // Create parallel download tasks (reuse existing logic)
        let mut tasks = Vec::new();
        for i in 0..PARALLEL_CONNECTIONS {
            let start = i as u64 * chunk_size;
            let end = if i == PARALLEL_CONNECTIONS - 1 {
                file_size - 1
            } else {
                (i as u64 + 1) * chunk_size - 1
            };

            let url_clone = url.clone();
            let target_path_clone = target_path.clone();
            let client = self.clone();
            let bytes_downloaded_clone = bytes_downloaded.clone();
            let semaphore = self.parallel_semaphore.clone();

            let task = tokio::spawn(async move {
                let _permit = semaphore.acquire().await
                    .map_err(|_| FileshareError::Transfer("Failed to acquire semaphore".to_string()))?;
                client.download_range(
                    url_clone,
                    target_path_clone,
                    start,
                    end,
                    bytes_downloaded_clone,
                ).await
            });

            tasks.push(task);
        }

        // Wait for all downloads to complete
        let results = join_all(tasks).await;
        download_complete.store(true, Ordering::Relaxed);
        monitor_handle.abort();

        // Check for errors
        for result in results {
            match result {
                Ok(Ok(())) => {},
                Ok(Err(e)) => return Err(e),
                Err(e) => return Err(FileshareError::Transfer(format!("Task failed: {}", e))),
            }
        }

        // Final callback with completion
        if let Some(ref callback) = *progress_callback {
            callback(file_size, file_size, 0);
        }

        // Ensure all data is written
        file.sync_all().await
            .map_err(|e| FileshareError::FileOperation(format!("Failed to sync: {}", e)))?;

        let duration = start_time.elapsed();
        let throughput_mbps = (file_size as f64 * 8.0) / (duration.as_secs_f64() * 1_000_000.0);

        info!("✅ Parallel HTTP download complete: {:.1} MB in {:.2}s ({:.1} Mbps)",
              file_size as f64 / (1024.0 * 1024.0),
              duration.as_secs_f64(),
              throughput_mbps);

        Ok(())
    }

    /// Download file using single connection with progress callback
    async fn download_file_single_with_progress(
        &self,
        url: String,
        target_path: PathBuf,
        expected_size: Option<u64>,
        progress_callback: Option<ProgressCallback>,
    ) -> Result<()> {
        let start_time = Instant::now();
        info!("⬇️ Starting HTTP download from: {}", url);

        let uri: Uri = url.parse()
            .map_err(|e| FileshareError::Transfer(format!("Invalid URL: {}", e)))?;

        let req = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .header("User-Agent", "FileshareHTTP/1.0")
            .body(http_body_util::Empty::<Bytes>::new())
            .map_err(|e| FileshareError::Transfer(format!("Failed to build request: {}", e)))?;

        let res = self.client.request(req).await
            .map_err(|e| FileshareError::Transfer(format!("Request failed: {}", e)))?;

        if !res.status().is_success() {
            return Err(FileshareError::Transfer(format!("HTTP error: {}", res.status())));
        }

        let content_length = res.headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());

        let file_size = content_length.or(expected_size).unwrap_or(0);
        info!("📊 Downloading {:.1} MB", file_size as f64 / (1024.0 * 1024.0));

        // Create target file
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&target_path)
            .await
            .map_err(|e| FileshareError::FileOperation(format!("Failed to create file: {}", e)))?;

        // Pre-allocate file if size is known
        if file_size > 0 {
            #[cfg(target_os = "linux")]
            {
                use std::os::unix::io::AsRawFd;
                let fd = file.as_raw_fd();
                unsafe {
                    let ret = libc::fallocate(fd, 0, 0, file_size as libc::off_t);
                    if ret != 0 {
                        file.set_len(file_size).await?;
                    }
                }
            }
            #[cfg(not(target_os = "linux"))]
            {
                file.set_len(file_size).await?;
            }
            file.seek(std::io::SeekFrom::Start(0)).await?;
        }

        // Stream body to file with progress callbacks
        let mut body = res.into_body();
        let mut buffer = Vec::with_capacity(DOWNLOAD_BUFFER_SIZE);
        let mut total_written = 0u64;
        let mut last_callback_time = Instant::now();
        let mut last_callback_bytes = 0u64;

        while let Some(chunk) = body.frame().await {
            let chunk = chunk.map_err(|e| FileshareError::Transfer(format!("Failed to read chunk: {}", e)))?;

            if let Some(data) = chunk.data_ref() {
                buffer.extend_from_slice(data);

                // Write more frequently for continuous streaming
                if buffer.len() >= WRITE_THRESHOLD {
                    file.write_all(&buffer).await
                        .map_err(|e| FileshareError::FileOperation(format!("Failed to write: {}", e)))?;

                    total_written += buffer.len() as u64;

                    // Call progress callback if enough time has passed (throttle to 10Hz)
                    let now = Instant::now();
                    if now.duration_since(last_callback_time).as_millis() >= 100 {
                        if let Some(ref callback) = progress_callback {
                            let elapsed = now.duration_since(last_callback_time).as_secs_f64();
                            let speed = if elapsed > 0.0 {
                                ((total_written - last_callback_bytes) as f64 / elapsed) as u64
                            } else {
                                0
                            };
                            callback(total_written, file_size, speed);
                        }
                        last_callback_time = now;
                        last_callback_bytes = total_written;
                    }

                    buffer.clear();
                }
            }
        }

        // Write any remaining data
        if !buffer.is_empty() {
            file.write_all(&buffer).await
                .map_err(|e| FileshareError::FileOperation(format!("Failed to write final chunk: {}", e)))?;
            total_written += buffer.len() as u64;
        }

        // Final progress callback
        if let Some(ref callback) = progress_callback {
            callback(total_written, file_size, 0);
        }

        // Ensure all data is written
        file.flush().await
            .map_err(|e| FileshareError::FileOperation(format!("Failed to flush: {}", e)))?;
        file.sync_all().await
            .map_err(|e| FileshareError::FileOperation(format!("Failed to sync: {}", e)))?;

        let duration = start_time.elapsed();
        let throughput_mbps = (total_written as f64 * 8.0) / (duration.as_secs_f64() * 1_000_000.0);

        info!("✅ HTTP download complete: {:.1} MB in {:.2}s ({:.1} Mbps)",
              total_written as f64 / (1024.0 * 1024.0),
              duration.as_secs_f64(),
              throughput_mbps);

        Ok(())
    }
}

impl Clone for HttpFileClient {
    fn clone(&self) -> Self {
        Self::new()
    }
}

// Add libc dependency for fallocate on Linux
#[cfg(target_os = "linux")]
extern crate libc;