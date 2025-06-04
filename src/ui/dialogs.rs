use crate::{network::peer::Peer, FileshareError, Result};
use rfd::FileDialog;
use std::path::PathBuf;
use tracing::{info, warn};
use uuid::Uuid;

pub async fn show_file_picker() -> Result<Option<Vec<PathBuf>>> {
    info!("Opening file picker");

    // Use blocking task since rfd is sync
    let result = tokio::task::spawn_blocking(|| {
        // Just use the simplest approach - no custom directory
        FileDialog::new()
            .set_title("Select files to share")
            .add_filter("All Files", &["*"])
            .pick_files()
    })
    .await;

    match result {
        Ok(Some(files)) => {
            info!("Selected {} files for sharing", files.len());
            for file in &files {
                info!("  - {:?}", file);
            }
            Ok(Some(files))
        }
        Ok(None) => {
            info!("File picker cancelled");
            Ok(None)
        }
        Err(e) => {
            warn!("File picker error: {}", e);
            Err(FileshareError::FileOperation(format!(
                "File picker failed: {}",
                e
            )))
        }
    }
}

pub async fn show_device_selector(peers: Vec<Peer>) -> Result<Option<Uuid>> {
    info!("Showing device selector for {} peers", peers.len());

    if peers.is_empty() {
        // Show notification that no devices are available
        show_notification(
            "No Devices Available",
            "No connected devices found. Make sure other devices are running the app and connected to the same network."
        ).await?;
        return Ok(None);
    }

    if peers.len() == 1 {
        // If only one device, auto-select it
        let peer = &peers[0];
        info!(
            "Auto-selecting single device: {} ({})",
            peer.device_info.name, peer.device_info.id
        );

        show_notification(
            "Sending to Device",
            &format!("Sending files to {}", peer.device_info.name),
        )
        .await?;

        return Ok(Some(peer.device_info.id));
    }

    // For now, show a simple notification and select the first device
    // In a future version, we could show a proper selection dialog
    let selected_peer = &peers[0];

    show_notification(
        "Multiple Devices Found",
        &format!(
            "Sending to {} (first available device)",
            selected_peer.device_info.name
        ),
    )
    .await?;

    info!(
        "Selected device: {} ({})",
        selected_peer.device_info.name, selected_peer.device_info.id
    );
    Ok(Some(selected_peer.device_info.id))
}

async fn show_notification(title: &str, message: &str) -> Result<()> {
    notify_rust::Notification::new()
        .summary(title)
        .body(message)
        .timeout(notify_rust::Timeout::Milliseconds(5000))
        .show()
        .map_err(|e| FileshareError::Unknown(format!("Notification error: {}", e)))?;

    Ok(())
}
