use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

#[derive(Debug, Clone)]
pub struct ClipboardState {
    pub files: Vec<PathBuf>,
    pub timestamp: u64,
    pub total_size: u64,
}

#[derive(Clone)]
pub struct ClipboardManager {
    state: Arc<RwLock<Option<ClipboardState>>>,
}

impl ClipboardManager {
    pub fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(None)),
        }
    }

    pub async fn copy_files(&self, files: Vec<PathBuf>) -> crate::Result<()> {
        info!("Copying {} files to clipboard", files.len());

        // Calculate total size
        let mut total_size = 0u64;
        for file in &files {
            if let Ok(metadata) = tokio::fs::metadata(file).await {
                total_size += metadata.len();
            }
        }

        let clipboard_state = ClipboardState {
            files,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            total_size,
        };

        {
            let mut state = self.state.write().await;
            *state = Some(clipboard_state.clone());
        }

        info!(
            "Clipboard updated: {} files, {} bytes",
            clipboard_state.files.len(),
            clipboard_state.total_size
        );

        Ok(())
    }

    pub async fn get_files(&self) -> Option<ClipboardState> {
        let state = self.state.read().await;
        state.clone()
    }

    pub async fn clear(&self) {
        info!("Clearing clipboard");
        let mut state = self.state.write().await;
        *state = None;
    }

    pub async fn is_empty(&self) -> bool {
        let state = self.state.read().await;
        state.is_none()
    }

    pub async fn get_file_count(&self) -> usize {
        let state = self.state.read().await;
        state.as_ref().map(|s| s.files.len()).unwrap_or(0)
    }
}
