use axum::{
    body::Body,
    extract::{Path, Request, State},
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
    Router,
};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs::File;
use tokio_util::io::ReaderStream;
use tower::ServiceBuilder;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::{debug, error, info, warn};
use uuid::Uuid;
use http_body_util::StreamBody;
use hyper::body::Frame;
use futures::stream::StreamExt;

use crate::{FileshareError, Result};

// Buffer size for streaming - optimized for LAN speeds
const STREAM_BUFFER_SIZE: usize = 64 * 1024 * 1024; // 64MB chunks for maximum HTTP streaming throughput

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

    /// Start the HTTP server
    pub async fn start(&self) -> Result<()> {
        let addr = SocketAddr::from(([0, 0, 0, 0], self.port));
        
        let app = Router::new()
            .route("/file/:token", get(Self::serve_file))
            .route("/health", get(|| async { "OK" }))
            .layer(
                ServiceBuilder::new()
                    .layer(TraceLayer::new_for_http())
                    .layer(CorsLayer::permissive())
            )
            .with_state(self.clone());

        let listener = tokio::net::TcpListener::bind(addr).await
            .map_err(|e| FileshareError::Network(e))?;
        
        info!("🌐 HTTP file server listening on {}", addr);

        axum::serve(listener, app).await
            .map_err(|e| FileshareError::Network(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("HTTP server error: {}", e)
            )))?;

        Ok(())
    }

    /// Serve a file using zero-copy streaming
    async fn serve_file(
        Path(token): Path<String>,
        State(server): State<HttpFileServer>,
        _request: Request,
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
        let filename = file_path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        info!("📤 Serving file via HTTP: {} ({:.1} MB)", 
              filename, file_size as f64 / (1024.0 * 1024.0));

        // Open file for streaming
        let file = match File::open(&file_path).await {
            Ok(f) => f,
            Err(e) => {
                error!("❌ Failed to open file: {}", e);
                return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to open file").into_response();
            }
        };

        // Create buffered reader for better performance
        let reader = tokio::io::BufReader::with_capacity(STREAM_BUFFER_SIZE, file);
        let stream = ReaderStream::new(reader);
        
        // Convert to hyper body stream
        let body_stream = stream.map(|result| {
            result.map(|bytes| Frame::data(bytes))
                  .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
        });
        
        let body = StreamBody::new(body_stream);

        // Set headers for optimal transfer
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_LENGTH, file_size.to_string().parse().unwrap());
        headers.insert(header::CONTENT_TYPE, "application/octet-stream".parse().unwrap());
        headers.insert(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", filename).parse().unwrap()
        );
        
        // Add cache control for better performance
        headers.insert(header::CACHE_CONTROL, "no-cache".parse().unwrap());
        
        (StatusCode::OK, headers, Body::new(body)).into_response()
    }

    /// Get server URL for a given token
    pub fn get_download_url(&self, token: &str, server_addr: &str) -> String {
        format!("http://{}:{}/file/{}", server_addr, self.port, token)
    }
}