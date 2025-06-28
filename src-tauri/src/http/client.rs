use bytes::Bytes;
use hyper::{Method, Request, Uri};
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::fs::OpenOptions;
use tokio::io::{AsyncWriteExt, AsyncSeekExt};
use tracing::{info, warn};
use http_body_util::BodyExt;

use crate::{FileshareError, Result};

// Download buffer size - optimized for LAN speeds
const DOWNLOAD_BUFFER_SIZE: usize = 32 * 1024 * 1024; // 32MB buffer

pub struct HttpFileClient {
    client: Client<hyper_util::client::legacy::connect::HttpConnector, http_body_util::Empty<Bytes>>,
}

impl HttpFileClient {
    pub fn new() -> Self {
        let client = Client::builder(TokioExecutor::new())
            .pool_idle_timeout(std::time::Duration::from_secs(30))
            .pool_max_idle_per_host(16) // Allow more connections for parallel downloads
            .http2_only(false) // Use HTTP/1.1 for better compatibility and streaming
            .build_http();
        
        Self { client }
    }

    /// Download a file from the given URL
    pub async fn download_file(
        &self,
        url: String,
        target_path: PathBuf,
        expected_size: Option<u64>,
    ) -> Result<()> {
        let start_time = Instant::now();
        info!("⬇️ Starting HTTP download from: {}", url);

        // Parse URL
        let uri: Uri = url.parse()
            .map_err(|e| FileshareError::Transfer(format!("Invalid URL: {}", e)))?;

        // Create HTTP request
        let req = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .header("User-Agent", "FileshareHTTP/1.0")
            .body(http_body_util::Empty::<Bytes>::new())
            .map_err(|e| FileshareError::Transfer(format!("Failed to build request: {}", e)))?;

        // Send request
        let res = self.client.request(req).await
            .map_err(|e| FileshareError::Transfer(format!("Request failed: {}", e)))?;

        if !res.status().is_success() {
            return Err(FileshareError::Transfer(format!("HTTP error: {}", res.status())));
        }

        // Get content length
        let content_length = res.headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());

        if let (Some(expected), Some(actual)) = (expected_size, content_length) {
            if expected != actual {
                warn!("⚠️ File size mismatch: expected {} bytes, got {} bytes", expected, actual);
            }
        }

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
            file.set_len(file_size).await
                .map_err(|e| FileshareError::FileOperation(format!("Failed to allocate file: {}", e)))?;
            file.seek(std::io::SeekFrom::Start(0)).await
                .map_err(|e| FileshareError::FileOperation(format!("Failed to seek: {}", e)))?;
        }

        // Download with progress tracking
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

        // Stream body to file
        let mut body = res.into_body();
        let mut buffer = Vec::with_capacity(DOWNLOAD_BUFFER_SIZE);
        let mut total_written = 0u64;

        while let Some(chunk) = body.frame().await {
            let chunk = chunk.map_err(|e| FileshareError::Transfer(format!("Failed to read chunk: {}", e)))?;
            
            if let Some(data) = chunk.data_ref() {
                buffer.extend_from_slice(data);
                
                // Write to file when buffer is large enough  
                if buffer.len() >= DOWNLOAD_BUFFER_SIZE {
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

    /// Download multiple files in parallel
    pub async fn download_files_parallel(
        &self,
        downloads: Vec<(String, PathBuf, Option<u64>)>, // (url, target_path, expected_size)
    ) -> Result<Vec<Result<()>>> {
        let handles: Vec<_> = downloads
            .into_iter()
            .map(|(url, path, size)| {
                let client = self.clone();
                tokio::spawn(async move {
                    client.download_file(url, path, size).await
                })
            })
            .collect();

        let mut results = Vec::new();
        for handle in handles {
            match handle.await {
                Ok(result) => results.push(result),
                Err(e) => results.push(Err(FileshareError::Transfer(format!("Task failed: {}", e)))),
            }
        }

        Ok(results)
    }
}

impl Clone for HttpFileClient {
    fn clone(&self) -> Self {
        Self::new() // Create new client instance for simplicity
    }
}