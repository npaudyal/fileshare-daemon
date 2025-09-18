# Feature Requirements: Native-Style File Transfer Progress Window

## Overview
Implement a standalone, native-style file transfer progress window that spawns when the paste hotkey is triggered, separate from the main application window. This window should provide real-time transfer progress feedback similar to native OS file copy dialogs.

## Core Requirements

### 1. Window Management
- **Separate Window**: Create a new Tauri window instance that is independent of the main application window
- **Window Properties**:
  - Size: 450x250px (initial), resizable with min size 400x200px
  - Position: Center of screen or near cursor position (if feasible)
  - Style: Frameless or minimal frame with custom titlebar
  - Always on top during active transfer
  - Semi-transparent background with blur effect (native OS style)
  - No system tray icon for this window

### 2. Transfer Progress UI

#### Visual Components
- **Header Section**:
  - Transfer direction indicator (icon + text: "Receiving file from [device_name]")
  - Close button (X) in top-right corner
  - Minimize button (optional)

- **File Information**:
  - File icon based on file type
  - File name (truncated with ellipsis if too long)
  - File size (human-readable format)
  - Source device name and icon

- **Progress Visualization**:
  - Progress bar showing percentage complete
  - Current transfer speed (MB/s)
  - Time remaining estimate
  - Bytes transferred / Total bytes
  - Real-time progress percentage

- **Action Buttons**:
  - Pause/Resume button
  - Cancel button
  - "Open file location" button (shown after completion)

### 3. Window Behavior

#### Lifecycle
- **Spawn Trigger**: Window opens immediately when paste hotkey is pressed
- **Auto-close Options**:
  - Close automatically 3 seconds after successful completion (configurable)
  - Keep open on error/cancellation until user dismisses
  - Option to keep window open after completion (checkbox)

#### States
- **Preparing**: "Preparing to receive file..."
- **Transferring**: Active progress display
- **Paused**: Show paused state with resume option
- **Completed**: Success message with file location
- **Error**: Error message with retry option
- **Cancelled**: Cancellation confirmation

Frontend (React)

Create new entry point: transfer-window.html and TransferWindow.tsx
Implement progress polling (every 100ms during active transfer)
Smooth progress bar animations
Handle window close events properly

5. User Experience
Visual Design

Match native OS transfer dialogs:

Windows: Windows Explorer copy dialog style
macOS: Finder copy progress style
Linux: File manager dependent (Nautilus/Dolphin style)


Smooth animations for progress updates
Subtle shadow and blur effects

Notifications

System notification on completion (if window was minimized)
Error sound on failure
Success sound on completion (optional)

6. Advanced Features
Multiple Transfers

Stack multiple progress bars if multiple paste operations happen
Single window with tabs or accordion for multiple transfers
Global pause/resume all transfers

Network Indicators

Connection quality indicator
Network speed throttling detection
Automatic retry on connection loss

8. Performance Considerations

Efficient progress updates (throttled to prevent UI lag)
Minimal memory footprint
Clean up resources on window close
Handle multiple simultaneous transfers efficiently

9. Error Handling

Graceful handling of connection loss
Clear error messages
Retry mechanism with exponential backoff
Proper cleanup on unexpected window close


Example User Flow

User selects file on Device A and presses copy hotkey
User switches to Device B and presses paste hotkey
Transfer progress window immediately appears showing "Preparing..."
Progress bar starts moving with real-time updates
Transfer completes, window shows success for 2 seconds
Window auto-closes