use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct NetworkClipboardItem {
    pub file_path: PathBuf,
    pub source_device: Uuid,
    pub timestamp: u64,
    pub file_size: u64,
}

#[derive(Clone)]
pub struct ClipboardManager {
    pub network_clipboard: Arc<RwLock<Option<NetworkClipboardItem>>>, // Make this public
    device_id: Uuid,
}

impl ClipboardManager {
    pub fn new(device_id: Uuid) -> Self {
        Self {
            network_clipboard: Arc::new(RwLock::new(None)),
            device_id,
        }
    }

    fn extract_filename_cross_platform(file_path: &PathBuf) -> String {
        let path_str = file_path.to_string_lossy();
        info!("Extracting filename from path: '{}'", path_str);

        // Handle both Windows and Unix paths manually for cross-platform compatibility
        let path_separators = ['/', '\\'];

        // Find the last path separator
        if let Some(last_sep_pos) = path_str.rfind(&path_separators[..]) {
            let filename = &path_str[last_sep_pos + 1..];
            info!("Extracted filename: '{}'", filename);
            filename.to_string()
        } else {
            // No separators found, the whole string is the filename
            info!(
                "No path separator found, using whole string as filename: '{}'",
                path_str
            );
            path_str.to_string()
        }
    }

    // Called when user hits copy hotkey - detect selected file and store in network clipboard
    pub async fn copy_selected_file(&self) -> crate::Result<()> {
        info!("Attempting to copy currently selected file");

        // Get the currently selected file from the OS
        let selected_file = self.get_selected_file_from_os().await?;

        if let Some(file_path) = selected_file {
            info!("Copying file to network clipboard: {:?}", file_path);

            // Get file info and validate
            let metadata = tokio::fs::metadata(&file_path).await.map_err(|e| {
                crate::FileshareError::FileOperation(format!("Cannot access file: {}", e))
            })?;

            // Validate file permissions and properties
            if metadata.is_dir() {
                return Err(crate::FileshareError::FileOperation(
                    "Cannot copy directories to clipboard".to_string(),
                ));
            }

            if metadata.len() == 0 {
                return Err(crate::FileshareError::FileOperation(
                    "Cannot copy empty files".to_string(),
                ));
            }

            // Check if file is readable
            match tokio::fs::File::open(&file_path).await {
                Ok(_) => {}
                Err(e) => {
                    return Err(crate::FileshareError::FileOperation(format!(
                        "File is not readable: {}",
                        e
                    )));
                }
            }

            info!("File validation passed: {} bytes", metadata.len());

            let clipboard_item = NetworkClipboardItem {
                file_path: file_path.clone(),
                source_device: self.device_id,
                timestamp: crate::utils::current_timestamp(),
                file_size: metadata.len(),
            };

            {
                let mut clipboard = self.network_clipboard.write().await;
                *clipboard = Some(clipboard_item);
            }

            info!("File copied to network clipboard: {:?}", file_path);

            // TODO: Broadcast to other devices that something was copied
            self.broadcast_clipboard_update().await?;
        } else {
            info!("No file selected in file manager");
            // Show notification that no file is selected
            self.show_notification(
                "Nothing Selected",
                "Please select a file in your file manager first",
            )
            .await?;
        }

        Ok(())
    }

    // Called when user hits paste hotkey - transfer file to current location
    pub async fn paste_to_current_location(&self) -> crate::Result<Option<(PathBuf, Uuid)>> {
        info!("Attempting to paste from network clipboard");

        let clipboard_item = {
            let clipboard = self.network_clipboard.read().await;
            clipboard.clone()
        };

        if let Some(item) = clipboard_item {
            // DEBUG: Log what's in the clipboard
            info!("DEBUG: Clipboard item file_path: {:?}", item.file_path);
            info!(
                "DEBUG: Clipboard item source_device: {}",
                item.source_device
            );

            // Don't paste on the same device that copied
            if item.source_device == self.device_id {
                info!("Ignoring paste on same device that copied");
                self.show_notification(
                    "Same Device",
                    "Can't paste on the same device you copied from",
                )
                .await?;
                return Ok(None);
            }

            // Get current directory where user wants to paste
            let target_dir = self.get_current_directory().await?;

            info!("Paste target directory: {:?}", target_dir);
            info!(
                "Will request file: {:?} from device: {}",
                item.file_path, item.source_device
            );

            // FIXED: Extract filename in a cross-platform way
            let filename = Self::extract_filename_cross_platform(&item.file_path);
            let target_path = target_dir.join(&filename);

            // DEBUG: Log the final paths
            info!("DEBUG: Final source path for request: {:?}", item.file_path);
            info!("DEBUG: Final target path for request: {:?}", target_path);

            // Return the file info and source device for the daemon to handle transfer
            Ok(Some((target_path, item.source_device)))
        } else {
            info!("Network clipboard is empty");
            self.show_notification(
                "Nothing to Paste",
                "Network clipboard is empty. Copy a file first.",
            )
            .await?;
            Ok(None)
        }
    }

    // Update clipboard when another device copies something
    pub async fn update_from_network(&self, item: NetworkClipboardItem) {
        info!(
            "Received network clipboard update from device {}: {:?}",
            item.source_device, item.file_path
        );
        let mut clipboard = self.network_clipboard.write().await;
        *clipboard = Some(item);
    }

    pub async fn clear(&self) {
        let mut clipboard = self.network_clipboard.write().await;
        *clipboard = None;
        info!("Network clipboard cleared");
    }

    pub async fn is_empty(&self) -> bool {
        let clipboard = self.network_clipboard.read().await;
        clipboard.is_none()
    }

    // Platform-specific: Get currently selected file in file manager
    async fn get_selected_file_from_os(&self) -> crate::Result<Option<PathBuf>> {
        #[cfg(target_os = "macos")]
        {
            self.get_selected_file_macos().await
        }
        #[cfg(target_os = "windows")]
        {
            self.get_selected_file_windows().await
        }
        #[cfg(target_os = "linux")]
        {
            self.get_selected_file_linux().await
        }
    }

    // Platform-specific: Get current directory in file manager
    async fn get_current_directory(&self) -> crate::Result<PathBuf> {
        #[cfg(target_os = "macos")]
        {
            self.get_current_directory_macos().await
        }
        #[cfg(target_os = "windows")]
        {
            self.get_current_directory_windows().await
        }
        #[cfg(target_os = "linux")]
        {
            self.get_current_directory_linux().await
        }
    }

    #[cfg(target_os = "macos")]
    async fn get_selected_file_macos(&self) -> crate::Result<Option<PathBuf>> {
        use std::process::Command;

        info!("Attempting to get selected file from Finder");

        // Fixed script that doesn't rely on count
        let script = r#"
        try
            tell application "Finder"
                set selectedItems to selection
                
                -- Try to get the first item directly
                try
                    set firstItem to item 1 of selectedItems
                    set itemKind to kind of firstItem
                    
                    -- Check if it's not a folder
                    if itemKind is not "Folder" then
                        set itemPath to POSIX path of (firstItem as alias)
                        return itemPath
                    else
                        return ""
                    end if
                on error
                    -- No items selected or error accessing first item
                    return ""
                end try
            end tell
        on error errMsg
            return ""
        end try
    "#;

        let output = Command::new("osascript")
            .arg("-e")
            .arg(script)
            .output()
            .map_err(|e| {
                crate::FileshareError::Unknown(format!("Failed to run AppleScript: {}", e))
            })?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let path_str = stdout.trim();
        let stderr_str = stderr.trim();

        info!("AppleScript output: '{}'", path_str);
        if !stderr_str.is_empty() {
            info!("AppleScript stderr: '{}'", stderr_str);
        }

        if !path_str.is_empty() && output.status.success() {
            let path = PathBuf::from(path_str);
            if path.exists() && path.is_file() {
                info!("Successfully detected selected file: {:?}", path);
                return Ok(Some(path));
            }
        }

        info!("No file selected or could not detect selection");
        Ok(None)
    }

    #[cfg(target_os = "macos")]
    async fn get_current_directory_macos(&self) -> crate::Result<PathBuf> {
        use std::process::Command;

        info!("Attempting to get current Finder directory");

        let script = r#"
        tell application "Finder"
            if (count of Finder windows) > 0 then
                set currentFolder to target of front Finder window
                return POSIX path of (currentFolder as alias)
            else
                return POSIX path of (desktop as alias)
            end if
        end tell
    "#;

        let output = Command::new("osascript")
            .arg("-e")
            .arg(script)
            .output()
            .map_err(|e| {
                crate::FileshareError::Unknown(format!("Failed to run AppleScript: {}", e))
            })?;

        let path_str = String::from_utf8_lossy(&output.stdout);
        let path_str = path_str.trim();
        info!("Current directory AppleScript output: '{}'", path_str);

        if !path_str.is_empty() && output.status.success() {
            let path = PathBuf::from(path_str);
            if path.exists() && path.is_dir() {
                info!("Current Finder directory: {:?}", path);
                return Ok(path);
            }
        }

        // Fallback to Desktop
        let desktop = dirs::desktop_dir().unwrap_or_else(|| PathBuf::from("/Users/Shared/Desktop"));
        info!("Using fallback directory: {:?}", desktop);
        Ok(desktop)
    }

    #[cfg(target_os = "windows")]
    async fn get_selected_file_windows(&self) -> crate::Result<Option<PathBuf>> {
        // Try the PowerShell approach first
        match self.get_selected_file_windows_powershell().await {
            Ok(Some(path)) => Ok(Some(path)),
            Ok(None) => {
                info!("No file selected in Windows Explorer");
                Ok(None)
            }
            Err(e) => {
                warn!("PowerShell method failed: {}, trying clipboard fallback", e);
                self.get_selected_file_windows_fallback().await
            }
        }
    }

    #[cfg(target_os = "windows")]
    async fn get_selected_file_windows_powershell(&self) -> crate::Result<Option<PathBuf>> {
        use std::process::Command;

        let script = r#"
        try {
            $shell = New-Object -ComObject Shell.Application
            $windows = $shell.Windows()
            
            foreach ($window in $windows) {
                if ($window.Name -match "Explorer" -and $window.Document) {
                    $selection = $window.Document.SelectedItems()
                    if ($selection.Count -gt 0) {
                        $item = $selection.Item(0)
                        if (-not $item.IsFolder) {
                            Write-Output $item.Path
                            exit 0
                        }
                    }
                }
            }
        } catch {
            # Silently fail and let the fallback handle it
        }
    "#;

        let output = tokio::process::Command::new("powershell")
            .arg("-WindowStyle")
            .arg("Hidden")
            .arg("-ExecutionPolicy")
            .arg("Bypass")
            .arg("-Command")
            .arg(script)
            .output()
            .await
            .map_err(|e| {
                crate::FileshareError::Unknown(format!("PowerShell execution failed: {}", e))
            })?;

        let path_str = String::from_utf8_lossy(&output.stdout);
        let path_str = path_str.trim();

        if !path_str.is_empty() && output.status.success() {
            let path = PathBuf::from(path_str);
            if path.exists() && path.is_file() {
                info!("Selected file detected via PowerShell: {:?}", path);
                return Ok(Some(path));
            }
        }

        Ok(None)
    }

    #[cfg(target_os = "windows")]
    async fn get_selected_file_windows_fallback(&self) -> crate::Result<Option<PathBuf>> {
        // Fallback: Check if user has recently copied a file path to clipboard
        // This is a simple fallback - not ideal but better than nothing
        info!("Using clipboard fallback for file detection on Windows");
        Ok(None)
    }

    #[cfg(target_os = "windows")]
    async fn get_current_directory_windows(&self) -> crate::Result<PathBuf> {
        // Try multiple approaches to get the active Explorer directory

        // Approach 1: PowerShell with better Explorer detection
        let script = r#"
        try {
            Add-Type -AssemblyName System.Windows.Forms
            $explorer = New-Object -ComObject Shell.Application
            $windows = $explorer.Windows()
            
            # Get the foreground window handle
            Add-Type @"
                using System;
                using System.Runtime.InteropServices;
                public class Win32 {
                    [DllImport("user32.dll")]
                    public static extern IntPtr GetForegroundWindow();
                }
"@
            $foregroundWindow = [Win32]::GetForegroundWindow()
            
            # Find the Explorer window that matches the foreground window
            foreach ($window in $windows) {
                try {
                    if ($window.HWND -eq $foregroundWindow.ToInt64()) {
                        $path = $window.Document.Folder.Self.Path
                        if ($path -and (Test-Path $path)) {
                            Write-Output $path
                            exit 0
                        }
                    }
                } catch {
                    # Skip this window if there's an error
                    continue
                }
            }
            
            # Fallback: get any open Explorer window
            foreach ($window in $windows) {
                try {
                    if ($window.Name -like "*Explorer*") {
                        $path = $window.Document.Folder.Self.Path
                        if ($path -and (Test-Path $path)) {
                            Write-Output $path
                            exit 0
                        }
                    }
                } catch {
                    continue
                }
            }
            
            # Final fallback: Documents folder
            Write-Output ([Environment]::GetFolderPath("MyDocuments"))
        } catch {
            # Ultimate fallback
            Write-Output ([Environment]::GetFolderPath("MyDocuments"))
        }
    "#;

        let output = tokio::process::Command::new("powershell")
            .arg("-WindowStyle")
            .arg("Hidden")
            .arg("-ExecutionPolicy")
            .arg("Bypass")
            .arg("-Command")
            .arg(script)
            .output()
            .await
            .map_err(|e| {
                crate::FileshareError::Unknown(format!("PowerShell execution failed: {}", e))
            })?;

        let path_str = String::from_utf8_lossy(&output.stdout);
        let path_str = path_str.trim();
        info!("Windows directory detection output: '{}'", path_str);

        if !path_str.is_empty() && output.status.success() {
            let path = PathBuf::from(path_str);
            if path.exists() && path.is_dir() {
                info!("Detected Windows directory: {:?}", path);
                return Ok(path);
            }
        }

        // Final fallback to Documents
        let documents =
            dirs::document_dir().unwrap_or_else(|| PathBuf::from("C:\\Users\\Public\\Documents"));
        warn!("Using fallback directory: {:?}", documents);
        Ok(documents)
    }

    #[cfg(target_os = "windows")]
    async fn get_current_directory_windows_powershell(&self) -> crate::Result<PathBuf> {
        use std::process::Command;

        let script = r#"
        try {
            $shell = New-Object -ComObject Shell.Application
            $windows = $shell.Windows()
            
            # Try to find the active Explorer window
            foreach ($window in $windows) {
                if ($window.Name -match "Explorer" -and $window.Document) {
                    $folder = $window.Document.Folder.Self.Path
                    if ($folder) {
                        Write-Output $folder
                        exit 0
                    }
                }
            }
            
            # Fallback to Desktop
            Write-Output ([Environment]::GetFolderPath("Desktop"))
        } catch {
            # Fallback to Desktop  
            Write-Output ([Environment]::GetFolderPath("Desktop"))
        }
    "#;

        let output = tokio::process::Command::new("powershell")
            .arg("-WindowStyle")
            .arg("Hidden")
            .arg("-ExecutionPolicy")
            .arg("Bypass")
            .arg("-Command")
            .arg(script)
            .output()
            .await
            .map_err(|e| {
                crate::FileshareError::Unknown(format!("PowerShell execution failed: {}", e))
            })?;

        let path_str = String::from_utf8_lossy(&output.stdout);
        let path_str = path_str.trim();
        if !path_str.is_empty() {
            let path = PathBuf::from(path_str);
            if path.exists() && path.is_dir() {
                info!("Current Explorer directory detected: {:?}", path);
                return Ok(path);
            }
        }

        // Final fallback
        Ok(dirs::desktop_dir().unwrap_or_else(|| PathBuf::from("C:\\Users\\Public\\Desktop")))
    }

    #[cfg(target_os = "windows")]
    async fn get_current_directory_windows_registry(&self) -> crate::Result<PathBuf> {
        use std::process::Command;

        // Get the last accessed folder from Windows Registry
        let output = Command::new("reg")
        .arg("query")
        .arg(r"HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\ComDlg32\LastVisitedPidlMRU")
        .arg("/v")
        .arg("MRUListEx")
        .output();

        if let Ok(output) = output {
            if output.status.success() {
                // Parse registry output to get recent directories
                // This is a simplified approach - in production you'd want more robust parsing
                let output_str = String::from_utf8_lossy(&output.stdout);
                info!("Registry output for recent directories: {}", output_str);
            }
        }

        // For now, fallback to Desktop - you can enhance this later
        Ok(dirs::desktop_dir().unwrap_or_else(|| PathBuf::from("C:\\Users\\Public\\Desktop")))
    }

    #[cfg(target_os = "linux")]
    async fn get_selected_file_linux(&self) -> crate::Result<Option<PathBuf>> {
        // TODO: Implement Linux file selection detection
        // This is complex because there are many file managers (Nautilus, Dolphin, etc.)
        Ok(None)
    }

    #[cfg(target_os = "linux")]
    async fn get_current_directory_linux(&self) -> crate::Result<PathBuf> {
        // TODO: Implement Linux current directory detection
        Ok(dirs::desktop_dir().unwrap_or_else(|| PathBuf::from(".")))
    }

    async fn broadcast_clipboard_update(&self) -> crate::Result<()> {
        // TODO: Implement network broadcast to tell other devices something was copied
        // This will integrate with the peer manager
        Ok(())
    }

    async fn show_notification(&self, title: &str, message: &str) -> crate::Result<()> {
        notify_rust::Notification::new()
            .summary(title)
            .body(message)
            .timeout(notify_rust::Timeout::Milliseconds(3000))
            .show()
            .map_err(|e| crate::FileshareError::Unknown(format!("Notification error: {}", e)))?;
        Ok(())
    }
}
