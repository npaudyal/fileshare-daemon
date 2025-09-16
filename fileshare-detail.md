# Fileshare Application - Detailed Technical Documentation

## Table of Contents
1. [Overview](#overview)
2. [Architecture](#architecture)
3. [Device Discovery Mechanism](#device-discovery-mechanism)
4. [Network Communication](#network-communication)
5. [File Transfer Process](#file-transfer-process)
6. [Clipboard Integration](#clipboard-integration)
7. [Data Persistence](#data-persistence)
8. [Security & Trust Model](#security--trust-model)
9. [User Interface](#user-interface)
10. [Application Flow](#application-flow)

## Overview

Fileshare is a cross-platform file sharing application built with:
- **Backend**: Rust with Tauri framework
- **Frontend**: React with TypeScript
- **Network**: QUIC protocol for control channel, HTTP for file transfers
- **Discovery**: UDP broadcast-based discovery on port 9877

The app allows devices on the same network to automatically discover each other and share files seamlessly through a clipboard-like interface.

## Architecture

### Technology Stack

```
┌─────────────────────────────────────┐
│         React Frontend              │
│    (TypeScript + Tailwind CSS)      │
└──────────────┬──────────────────────┘
               │ Tauri IPC
┌──────────────▼──────────────────────┐
│         Tauri Runtime               │
│         (Rust Backend)              │
├─────────────────────────────────────┤
│  • FileshareDaemon (Core Service)  │
│  • PeerManager (Connection Mgmt)   │
│  • DiscoveryService (mDNS/UDP)     │
│  • HttpTransferManager (Files)     │
│  • ClipboardManager (OS Clipboard) │
│  • HotkeyManager (Global Hotkeys)  │
└─────────────────────────────────────┘
```

### Key Components

1. **FileshareDaemon** (`src-tauri/src/service/daemon_quic.rs`)
   - Central service orchestrator
   - Manages all background services
   - Handles lifecycle and coordination

2. **PeerManager** (`src-tauri/src/network/peer_quic.rs`)
   - Manages QUIC connections with peers
   - Handles authentication and handshakes
   - Maintains peer health monitoring
   - Routes messages between devices

3. **DiscoveryService** (`src-tauri/src/network/discovery.rs`)
   - UDP broadcast for device announcement
   - Listens for other devices on the network
   - Maintains discovered devices list

4. **HttpTransferManager** (`src-tauri/src/http/transfer.rs`)
   - HTTP server for file hosting
   - HTTP client for file downloading
   - Manages active transfers and progress

## Device Discovery Mechanism

### How Devices Find Each Other

1. **Broadcast Announcement** (Every 5 seconds)
   ```json
   {
     "device_id": "uuid-here",
     "device_name": "MacBook Pro",
     "port": 9876,        // QUIC port
     "version": "0.1.0",
     "timestamp": 1234567890
   }
   ```
   - Sent to `255.255.255.255:9877` (broadcast address)
   - Contains device metadata and connection info

2. **Discovery Listener**
   - Binds to `0.0.0.0:9877`
   - Receives broadcast packets from other devices
   - Filters out self-discovery packets
   - Updates discovered devices map

3. **Automatic Connection Initiation**
   - When a device is discovered, PeerManager is notified
   - Attempts QUIC connection to `device_ip:port`
   - Performs handshake and authentication

### Discovery Flow
```
Device A                    Network                     Device B
   │                                                        │
   ├──────────────────► UDP Broadcast ──────────────────►  │
   │                    (255.255.255.255:9877)             │
   │                                                        │
   │  ◄────────────────── UDP Broadcast ◄──────────────────┤
   │                                                        │
   ├──────────────────► QUIC Connection ─────────────────► │
   │                    (device_b_ip:9876)                 │
   │                                                        │
   │  ◄──────────────── Handshake/Auth ◄───────────────────┤
   │                                                        │
```

## Network Communication

### QUIC Protocol (Control Channel)

**Purpose**: Control messages, handshakes, file transfer requests

**Port**: 9876 (configurable)

**Key Features**:
- Low latency for control messages
- Multiplexed streams
- Built-in encryption
- Connection health monitoring (ping every 30s)

**Message Types**:
```rust
enum Message {
    Handshake { device_id, device_name, version },
    HandshakeResponse { device_id, device_name, version },
    FileOffer { file_path, file_size, download_url },
    FileRequest { request_id, file_path, target_path },
    ClipboardUpdate { file_path, timestamp },
    Ping,
    Pong,
    Error { message }
}
```

### HTTP Protocol (File Transfers)

**Purpose**: Actual file data transfer

**Port**: 9878 (configurable)

**Why HTTP instead of QUIC for files?**
- Better performance for large file transfers
- Support for parallel chunk downloads
- Resume capability
- Browser compatibility

**Transfer Process**:
1. File registered with HTTP server, gets unique token
2. Download URL generated: `http://device_ip:9878/download/{token}`
3. URL sent to recipient via QUIC
4. Recipient downloads file via HTTP

## File Transfer Process

### Complete Transfer Flow

1. **Sender Side (Copy)**
   ```
   User selects file → Hotkey (Cmd+Shift+Y) → ClipboardManager
        ↓
   File validated (exists, readable, not directory)
        ↓
   NetworkClipboardItem created with metadata
        ↓
   Broadcast clipboard update to all peers
   ```

2. **Receiver Side (Paste)**
   ```
   User triggers paste → Hotkey (Cmd+Shift+I) → ClipboardManager
        ↓
   Check network clipboard for available file
        ↓
   Request file from source device via QUIC
        ↓
   Receive download URL in response
        ↓
   Download file via HTTP to target location
        ↓
   File saved to current directory
   ```

### Transfer Optimizations

- **Adaptive chunk size**: Adjusts based on network speed
- **Parallel chunks**: Up to 8 concurrent chunk downloads
- **TCP optimizations**: Custom socket settings for performance
- **Large file detection**: Special handling for files >50MB
- **Resume support**: Can resume interrupted transfers

## Clipboard Integration

### Network Clipboard Concept

The app implements a "network clipboard" that works across devices:

1. **Copy Operation**
   - User selects file in OS file manager
   - Presses global hotkey (Cmd+Shift+Y on macOS)
   - File path extracted from OS selection
   - File metadata stored in network clipboard
   - All connected devices notified

2. **Paste Operation**
   - User navigates to target directory
   - Presses global hotkey (Cmd+Shift+I on macOS)
   - Checks if network clipboard has content
   - Verifies source device is different
   - Initiates file transfer to current directory

### Platform-Specific Implementation

**macOS**:
- Uses AppleScript to get selected files from Finder
- Uses `pwd` command to get current directory

**Windows**:
- Uses PowerShell to interact with Explorer
- Registry-based file selection detection

**Linux**:
- X11/Wayland clipboard monitoring
- File manager integration varies by DE

## Data Persistence

### Configuration Storage

**Location**: Platform-specific config directories
- macOS: `~/Library/Application Support/com.fileshare.daemon/`
- Windows: `%APPDATA%\fileshare\daemon\`
- Linux: `~/.config/fileshare-daemon/`

**Config File**: `settings.toml`
```toml
[device]
id = "uuid-v4-here"
name = "MacBook Pro"
icon = "laptop"

[network]
port = 9876           # QUIC port
discovery_port = 9877 # UDP discovery
http_port = 9878      # HTTP transfers
timeout_seconds = 30

[transfer]
chunk_size = 1048576  # 1MB
max_concurrent_transfers = 5
parallel_chunks = 8
adaptive_chunk_size = true
compression_enabled = false
resume_enabled = true

[security]
require_pairing = false    # Currently not enforced
encryption_enabled = true
allowed_devices = []        # List of trusted device UUIDs
```

### Device State Management

The app maintains several in-memory states:

1. **Discovered Devices Map**
   - All devices found on network
   - Updated by discovery service
   - Includes last_seen timestamp

2. **Peer Connections Map**
   - Active QUIC connections
   - Connection health status
   - Authentication state

3. **Active Transfers Map**
   - Ongoing file transfers
   - Progress tracking
   - Speed metrics

## Security & Trust Model

### Current Security Implementation

1. **Device Identification**
   - Each device has unique UUID
   - Persisted in config file
   - Used for all communications

2. **Connection Security**
   - QUIC provides encrypted transport
   - TLS 1.3 for handshakes
   - Self-signed certificates (currently)

3. **Allowed Devices List**
   - Whitelist of trusted device UUIDs
   - Stored in `security.allowed_devices`
   - Currently NOT enforced (all devices allowed)

### Trust Levels (Defined but Not Implemented)

```rust
enum TrustLevel {
    Unknown,    // Newly discovered
    Trusted,    // User approved
    Verified,   // PIN verified (future)
    Blocked     // Explicitly blocked
}
```

### Missing Security Features

- No pairing mechanism (despite config flag)
- No PIN verification
- No device authentication beyond UUID
- All discovered devices automatically trusted
- No encryption for file content (relies on QUIC/TLS)

## User Interface

### Main Components

1. **App.tsx** - Main application container
   - State management for devices and settings
   - Tab navigation (Devices, Settings, Info)
   - Global event handlers

2. **DevicesList.tsx** - Device display
   - Shows all discovered devices
   - Filter/search capabilities
   - Device actions (pair, block, rename)

3. **DeviceCard.tsx** - Individual device
   - Connection status indicator
   - Device metadata display
   - Action buttons

4. **Navigation.tsx** - Tab navigation
   - Devices tab (main view)
   - Settings tab (configuration)
   - Info tab (system information)

### State Management

- Local React state (useState)
- No global state management library
- Props drilling for component communication
- Tauri IPC for backend communication

### UI Features

- Real-time device discovery updates
- Connection status indicators
- Bulk device management
- Search and filter devices
- Transfer progress display
- Toast notifications

## Application Flow

### Startup Sequence

1. **Main Process** (`main.rs`)
   ```
   Load settings → Create daemon → Start Tauri app
        ↓              ↓              ↓
   Validate config  Init services  Create window
                         ↓
                  Start background tasks
   ```

2. **Service Initialization**
   ```
   FileshareDaemon::new()
        ├── PeerManager (QUIC listener)
        ├── DiscoveryService (UDP broadcast/listen)
        ├── HttpTransferManager (HTTP server)
        ├── ClipboardManager (OS integration)
        └── HotkeyManager (Global hotkeys)
   ```

3. **Discovery Loop**
   ```
   Every 5 seconds:
   Broadcast presence → Receive broadcasts → Update device list
                              ↓
                     Initiate QUIC connections
                              ↓
                        Perform handshake
                              ↓
                      Mark device as connected
   ```

### File Transfer Sequence

```
Device A (Sender)                          Device B (Receiver)
       │                                           │
   [User copies file]                              │
       ↓                                           │
   Store in network clipboard                      │
       ↓                                           │
   Broadcast ClipboardUpdate ──────────────────►  │
                                                   │
                                          [User pastes file]
                                                   ↓
       │  ◄──────────────────────── FileRequest message
       ↓
   Prepare file for HTTP transfer
       ↓
   Generate download URL
       ↓
   Send FileOffer with URL ────────────────────►  │
                                                   ↓
                                          Download via HTTP
                                                   ↓
                                           Save to target dir
```

### Connection Health Monitoring

```
Every 30 seconds:
   For each peer:
      Send Ping message
           ↓
      Wait for Pong (10s timeout)
           ↓
      If no response after 3 attempts:
           ↓
      Mark as disconnected
           ↓
      Attempt reconnection (max 5 attempts)
```

## Key Observations

### Strengths
1. Clean separation of concerns (QUIC for control, HTTP for data)
2. Automatic device discovery works reliably
3. Good performance optimizations for transfers
4. Cross-platform clipboard integration
5. Real-time connection monitoring

### Current Limitations
1. **No pairing mechanism** - All devices automatically trusted
2. **No authentication** - Only device UUID verification
3. **No encrypted file storage** - Files transferred in clear over HTTP
4. **Limited error recovery** - Some edge cases not handled
5. **No transfer queue** - Direct transfer only
6. **Single file transfers** - No folder or multi-file support

### Security Concerns
1. Any device on network can receive files
2. No verification of device identity
3. Broadcast packets reveal device presence
4. File URLs potentially guessable (uses UUID tokens)
5. No audit logging of transfers

## How Devices Are Added to Allowed List

Currently, the allowed devices mechanism is **defined but not enforced**:

1. **Automatic Discovery**
   - All devices discovered via UDP broadcast
   - Added to `discovered_devices` map
   - No filtering based on allowed list

2. **Connection Acceptance**
   - All incoming QUIC connections accepted
   - Handshake performs basic validation
   - Device UUID exchanged but not verified against allowed list

3. **File Transfer Permission**
   - Any connected device can send/receive files
   - No permission checks beyond connection status
   - `allowed_devices` list in config is ignored

4. **Future Pairing Flow** (Not Implemented)
   ```
   Discovery → User approval → PIN exchange → Add to allowed_devices
                                    ↓
                            Store in config file
                                    ↓
                            Future connections auto-trusted
   ```

## Summary

Fileshare is a well-architected file sharing application with solid networking foundations but currently lacks the security layer needed for production use. The codebase is ready for implementing a pairing mechanism - all the hooks are in place (device management UI, allowed_devices config, trust levels) but the actual enforcement and pairing flow need to be implemented.

The app excels at:
- Fast, reliable device discovery
- Efficient file transfers
- Cross-platform compatibility
- Clean code architecture

But needs improvement in:
- Device authentication and pairing
- Security and access control
- Error handling and recovery
- Multi-file transfer support