use axum::{
    body::Body,
    extract::{Path, Request, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::IntoResponse,
    routing::get,
    Router,
};
use futures::stream::StreamExt;
use http_body_util::StreamBody;
use hyper::body::Frame;
use std::io::SeekFrom;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt, BufReader};
use tokio_util::io::ReaderStream;
use tower::ServiceBuilder;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::{FileshareError, Result};
use super::optimization::{FileCategory, TransferProfile};

#[derive(Clone)]
pub struct OptimizedHttpServer {
    port: u16,
    allowed_paths: Arc<dashmap::DashMap<String, (PathBuf, u64)>>, // token -> (path, size)
}

impl OptimizedHttpServer {
    pub fn new(port: u16) -> Self {
        Self {
            port,
            allowed_paths: Arc::new(dashmap::DashMap::new()),
        }
    }

    pub async fn register_file(&self, file_path: PathBuf) -> Result<String> {
        // Get file size for optimization decisions
        let metadata = tokio::fs::metadata(&file_path).await
            .map_err(|e| FileshareError::FileOperation(format!("Failed to get metadata: {}", e)))?;
        
        let file_size = metadata.len();
        let token = Uuid::new_v4().to_string();
        
        self.allowed_paths.insert(token.clone(), (file_path, file_size));
        
        let category = FileCategory::from_size(file_size);
        info!("📝 Registered file for optimized HTTP access: token={}, size={:.1}MB, category={:?}", 
              token, file_size as f64 / (1024.0 * 1024.0), category);
        
        Ok(token)
    }

    pub fn unregister_file(&self, token: &str) {
        self.allowed_paths.remove(token);
        debug!("🗑️ Unregistered file token: {}", token);
    }

    pub async fn start_optimized(&self) -> Result<()> {
        let addr = SocketAddr::from(([0, 0, 0, 0], self.port));

        let app = Router::new()
            .route("/file/:token", get(Self::serve_file_optimized))
            .route("/health", get(|| async { "OK" }))
            .layer(
                ServiceBuilder::new()
                    .layer(TraceLayer::new_for_http())
                    .layer(CorsLayer::permissive()),
            )
            .with_state(self.clone());

        // Create optimized TCP listener
        let listener = self.create_optimized_listener(addr).await?;

        info!("🚀 Optimized HTTP file server listening on {} with dynamic profiles", addr);

        axum::serve(listener, app).await.map_err(|e| {
            FileshareError::Network(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("HTTP server error: {}", e),
            ))
        })?;

        Ok(())
    }

    async fn create_optimized_listener(&self, addr: SocketAddr) -> Result<tokio::net::TcpListener> {
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| FileshareError::Network(e))?;

        // Apply maximum performance socket options
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let fd = listener.as_raw_fd();
            unsafe {
                // Maximum send/receive buffers (32MB)
                let max_buf_size = 32 * 1024 * 1024;
                libc::setsockopt(
                    fd,
                    libc::SOL_SOCKET,
                    libc::SO_SNDBUF,
                    &max_buf_size as *const _ as *const libc::c_void,
                    std::mem::size_of::<libc::c_int>() as libc::socklen_t,
                );
                
                libc::setsockopt(
                    fd,
                    libc::SOL_SOCKET,
                    libc::SO_RCVBUF,
                    &max_buf_size as *const _ as *const libc::c_void,
                    std::mem::size_of::<libc::c_int>() as libc::socklen_t,
                );

                // Enable SO_REUSEADDR
                let reuse = 1;
                libc::setsockopt(
                    fd,
                    libc::SOL_SOCKET,
                    libc::SO_REUSEADDR,
                    &reuse as *const _ as *const libc::c_void,
                    std::mem::size_of::<libc::c_int>() as libc::socklen_t,
                );

                // TCP_NODELAY for low latency
                let nodelay = 1;
                libc::setsockopt(
                    fd,
                    libc::IPPROTO_TCP,
                    libc::TCP_NODELAY,
                    &nodelay as *const _ as *const libc::c_void,
                    std::mem::size_of::<libc::c_int>() as libc::socklen_t,
                );

                // TCP_QUICKACK for faster ACKs
                #[cfg(target_os = "linux")]
                {
                    let quickack = 1;
                    libc::setsockopt(
                        fd,
                        libc::IPPROTO_TCP,
                        libc::TCP_QUICKACK,
                        &quickack as *const _ as *const libc::c_void,
                        std::mem::size_of::<libc::c_int>() as libc::socklen_t,
                    );
                }
            }
        }

        Ok(listener)
    }

    async fn serve_file_optimized(
        Path(token): Path<String>,
        State(server): State<OptimizedHttpServer>,
        request: Request,
    ) -> impl IntoResponse {
        // Get file info
        let (file_path, file_size) = match server.allowed_paths.get(&token) {
            Some(entry) => entry.value().clone(),
            None => {
                warn!("❌ Invalid file token: {}", token);
                return (StatusCode::NOT_FOUND, "File not found").into_response();
            }
        };

        // Determine optimal serving profile
        let category = FileCategory::from_size(file_size);
        let profile = category.optimal_profile();

        // Get file metadata
        let metadata = match tokio::fs::metadata(&file_path).await {
            Ok(m) => m,
            Err(e) => {
                error!("❌ Failed to get file metadata: {}", e);
                return (StatusCode::NOT_FOUND, "File not found").into_response();
            }
        };

        let filename = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        // Handle range requests
        let range_header = request.headers().get("range");
        let (start, end) = if let Some(range_value) = range_header {
            match Self::parse_range_header(range_value, file_size) {
                Ok(range) => range,
                Err(_) => {
                    return (StatusCode::RANGE_NOT_SATISFIABLE, "Invalid range").into_response();
                }
            }
        } else {
            (0, file_size - 1)
        };

        let content_length = end - start + 1;

        info!(
            "📤 Serving file via optimized HTTP: {} ({:.1} MB) - Range: {}-{} - Profile: {:?}",
            filename,
            file_size as f64 / (1024.0 * 1024.0),
            start,
            end,
            category
        );

        // Open and prepare file
        let mut file = match File::open(&file_path).await {
            Ok(f) => f,
            Err(e) => {
                error!("❌ Failed to open file: {}", e);
                return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to open file").into_response();
            }
        };

        if start > 0 {
            if let Err(e) = file.seek(SeekFrom::Start(start)).await {
                error!("❌ Failed to seek: {}", e);
                return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to seek").into_response();
            }
        }

        // Create optimized reader based on file category
        let reader = Self::create_optimized_reader(file, content_length, profile).await;
        let stream = ReaderStream::with_capacity(reader, profile.buffer_size);

        // Convert to hyper body stream
        let body_stream = stream.map(|result| {
            result
                .map(|bytes| Frame::data(bytes))
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
        });

        let body = StreamBody::new(body_stream);

        // Optimize headers based on file size
        let mut headers = Self::create_optimized_headers(
            start,
            end,
            file_size,
            content_length,
            filename,
            range_header.is_some(),
            &profile,
        );

        let status = if range_header.is_some() {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        };

        (status, headers, Body::new(body)).into_response()
    }

    fn create_optimized_headers(
        start: u64,
        end: u64,
        file_size: u64,
        content_length: u64,
        filename: &str,
        is_range: bool,
        profile: &TransferProfile,
    ) -> HeaderMap {
        let mut headers = HeaderMap::new();

        if is_range {
            headers.insert(
                header::CONTENT_RANGE,
                HeaderValue::from_str(&format!("bytes {}-{}/{}", start, end, file_size)).unwrap(),
            );
            headers.insert(
                header::CONTENT_LENGTH,
                content_length.to_string().parse().unwrap(),
            );
        } else {
            headers.insert(
                header::CONTENT_LENGTH,
                file_size.to_string().parse().unwrap(),
            );
        }

        headers.insert(
            header::CONTENT_TYPE,
            "application/octet-stream".parse().unwrap(),
        );
        headers.insert(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", filename)
                .parse()
                .unwrap(),
        );

        // Performance headers
        headers.insert(header::CACHE_CONTROL, "no-cache, no-store".parse().unwrap());
        headers.insert(header::ACCEPT_RANGES, "bytes".parse().unwrap());
        headers.insert("X-Accel-Buffering", "no".parse().unwrap());

        // Add TCP hints
        if profile.tcp_buffer_size >= 16 * 1024 * 1024 {
            headers.insert("X-Large-File", "1".parse().unwrap());
        }

        // Connection handling
        headers.insert(header::CONNECTION, "keep-alive".parse().unwrap());
        headers.insert("Keep-Alive", "timeout=30, max=100".parse().unwrap());

        headers
    }

    async fn create_optimized_reader(
        file: File,
        content_length: u64,
        profile: TransferProfile,
    ) -> Box<dyn tokio::io::AsyncRead + Send + Unpin> {
        // Use appropriate buffering based on profile
        if profile.buffer_size >= 8 * 1024 * 1024 {
            // Large buffers for big files
            Box::new(BufReader::with_capacity(
                profile.buffer_size,
                file.take(content_length),
            ))
        } else {
            // Smaller buffers for small files
            Box::new(BufReader::with_capacity(
                profile.buffer_size.max(64 * 1024),
                file.take(content_length),
            ))
        }
    }

    fn parse_range_header(range_value: &HeaderValue, file_size: u64) -> Result<(u64, u64)> {
        let range_str = range_value
            .to_str()
            .map_err(|_| FileshareError::Transfer("Invalid range header".to_string()))?;

        if !range_str.starts_with("bytes=") {
            return Err(FileshareError::Transfer("Invalid range format".to_string()));
        }

        let range_spec = &range_str[6..];
        let parts: Vec<&str> = range_spec.split('-').collect();

        if parts.len() != 2 {
            return Err(FileshareError::Transfer("Invalid range format".to_string()));
        }

        let start = if parts[0].is_empty() {
            let suffix_length: u64 = parts[1]
                .parse()
                .map_err(|_| FileshareError::Transfer("Invalid range".to_string()))?;
            file_size.saturating_sub(suffix_length)
        } else {
            parts[0]
                .parse()
                .map_err(|_| FileshareError::Transfer("Invalid range start".to_string()))?
        };

        let end = if parts[1].is_empty() {
            file_size - 1
        } else {
            parts[1]
                .parse::<u64>()
                .map_err(|_| FileshareError::Transfer("Invalid range end".to_string()))?
                .min(file_size - 1)
        };

        if start > end || start >= file_size {
            return Err(FileshareError::Transfer("Range out of bounds".to_string()));
        }

        Ok((start, end))
    }

    pub fn get_download_url(&self, token: &str, server_addr: &str) -> String {
        format!("http://{}:{}/file/{}", server_addr, self.port, token)
    }
}

#[cfg(unix)]
extern crate libc;