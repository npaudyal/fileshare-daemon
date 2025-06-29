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
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;
use tower::ServiceBuilder;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::{FileshareError, Result};

// Optimized buffer size for streaming - tuned for LAN speeds
const STREAM_BUFFER_SIZE: usize = 16 * 1024 * 1024; // 8MB chunks for optimal streaming
const TCP_BUFFER_SIZE: u32 = 16 * 1024 * 1024; // 4MB TCP buffer

#[derive(Clone)]
pub struct HttpFileServer {
    port: u16,
    allowed_paths: Arc<dashmap::DashMap<String, PathBuf>>, // token -> file path mapping
}

impl HttpFileServer {
    pub fn new(port: u16) -> Self {
        Self {
            port,
            allowed_paths: Arc::new(dashmap::DashMap::new()),
        }
    }

    /// Register a file for HTTP access and return the access token
    pub fn register_file(&self, file_path: PathBuf) -> String {
        let token = Uuid::new_v4().to_string();
        self.allowed_paths.insert(token.clone(), file_path);
        info!("📝 Registered file for HTTP access with token: {}", token);
        token
    }

    /// Unregister a file (cleanup after transfer)
    pub fn unregister_file(&self, token: &str) {
        self.allowed_paths.remove(token);
        debug!("🗑️ Unregistered file token: {}", token);
    }

    /// Start the HTTP server with optimizations
    pub async fn start(&self) -> Result<()> {
        let addr = SocketAddr::from(([0, 0, 0, 0], self.port));

        let app = Router::new()
            .route("/file/:token", get(Self::serve_file))
            .route("/health", get(|| async { "OK" }))
            .layer(
                ServiceBuilder::new()
                    .layer(TraceLayer::new_for_http())
                    .layer(CorsLayer::permissive()),
            )
            .with_state(self.clone());

        // Create TCP listener with optimized settings
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| FileshareError::Network(e))?;

        // Configure socket options for better performance
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let fd = listener.as_raw_fd();
            unsafe {
                // Set send buffer size
                let send_buf_size = TCP_BUFFER_SIZE as libc::c_int;
                libc::setsockopt(
                    fd,
                    libc::SOL_SOCKET,
                    libc::SO_SNDBUF,
                    &send_buf_size as *const _ as *const libc::c_void,
                    std::mem::size_of::<libc::c_int>() as libc::socklen_t,
                );

                // Set receive buffer size
                let recv_buf_size = TCP_BUFFER_SIZE as libc::c_int;
                libc::setsockopt(
                    fd,
                    libc::SOL_SOCKET,
                    libc::SO_RCVBUF,
                    &recv_buf_size as *const _ as *const libc::c_void,
                    std::mem::size_of::<libc::c_int>() as libc::socklen_t,
                );
            }
        }

        info!(
            "🌐 HTTP file server listening on {} with optimized settings",
            addr
        );

        axum::serve(listener, app).await.map_err(|e| {
            FileshareError::Network(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("HTTP server error: {}", e),
            ))
        })?;

        Ok(())
    }

    /// Serve a file using optimized streaming with range support
    async fn serve_file(
        Path(token): Path<String>,
        State(server): State<HttpFileServer>,
        request: Request,
    ) -> impl IntoResponse {
        // Validate token and get file path
        let file_path = match server.allowed_paths.get(&token) {
            Some(entry) => entry.value().clone(),
            None => {
                warn!("❌ Invalid file token: {}", token);
                return (StatusCode::NOT_FOUND, "File not found").into_response();
            }
        };

        // Get file metadata
        let metadata = match tokio::fs::metadata(&file_path).await {
            Ok(m) => m,
            Err(e) => {
                error!("❌ Failed to get file metadata: {}", e);
                return (StatusCode::NOT_FOUND, "File not found").into_response();
            }
        };

        let file_size = metadata.len();
        let filename = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        // Check for range header (support for parallel downloads)
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
            "📤 Serving file via HTTP: {} ({:.1} MB) - Range: {}-{}",
            filename,
            file_size as f64 / (1024.0 * 1024.0),
            start,
            end
        );

        // Open file for streaming
        let mut file = match File::open(&file_path).await {
            Ok(f) => f,
            Err(e) => {
                error!("❌ Failed to open file: {}", e);
                return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to open file").into_response();
            }
        };

        // Seek to start position if needed
        if start > 0 {
            if let Err(e) = file.seek(SeekFrom::Start(start)).await {
                error!("❌ Failed to seek: {}", e);
                return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to seek").into_response();
            }
        }

        // Create optimized reader for streaming
        let reader = Self::create_zero_copy_reader(file, content_length).await;

        let stream = ReaderStream::with_capacity(reader, STREAM_BUFFER_SIZE);

        // Convert to hyper body stream
        let body_stream = stream.map(|result| {
            result
                .map(|bytes| Frame::data(bytes))
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
        });

        let body = StreamBody::new(body_stream);

        // Set headers for optimal transfer
        let mut headers = HeaderMap::new();

        if range_header.is_some() {
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

        // Add headers for better performance
        headers.insert(header::CACHE_CONTROL, "no-cache, no-store".parse().unwrap());
        headers.insert(header::ACCEPT_RANGES, "bytes".parse().unwrap());

        // Enable TCP_NODELAY for low latency
        headers.insert("X-Accel-Buffering", "no".parse().unwrap());

        let status = if range_header.is_some() {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        };

        (status, headers, Body::new(body)).into_response()
    }

    /// Parse HTTP Range header
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
            // Suffix range (e.g., "-500" means last 500 bytes)
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
            // Open-ended range (e.g., "500-" means from 500 to end)
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

    /// Create optimized reader for streaming
    async fn create_zero_copy_reader(
        file: File,
        content_length: u64,
    ) -> Box<dyn tokio::io::AsyncRead + Send + Unpin> {
        // Use buffered reader with large buffer for optimal streaming
        Box::new(tokio::io::BufReader::with_capacity(
            STREAM_BUFFER_SIZE,
            file.take(content_length),
        ))
    }

    /// Get server URL for a given token
    pub fn get_download_url(&self, token: &str, server_addr: &str) -> String {
        format!("http://{}:{}/file/{}", server_addr, self.port, token)
    }
}

// Add libc dependency for socket options on Unix
#[cfg(unix)]
extern crate libc;
