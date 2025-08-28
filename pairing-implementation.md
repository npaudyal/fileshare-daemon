# Pairing Mechanism Implementation Plan

## Executive Summary

This document outlines a comprehensive implementation plan for adding a secure pairing mechanism to the Fileshare Daemon application. The system currently auto-connects devices on the same network, but we will implement a secure PIN-based pairing system that requires explicit authorization before devices can share files.

## Current Architecture Analysis

### Existing Components

1. **Backend (Rust/Tauri)**
   - `FileshareDaemon`: Main service orchestrator
   - `PeerManager`: Manages QUIC connections and peer states
   - `DiscoveryService`: UDP broadcast-based device discovery
   - `Settings`: Configuration management with security settings
   - QUIC protocol for secure, fast data transfer
   - HTTP/2 for large file transfers

2. **Frontend (React/TypeScript)**
   - Tab-based navigation (Devices, Settings, Info)
   - Device management UI with pairing/unpairing capabilities
   - Real-time device discovery display
   - Toast notification system

3. **Current Flow**
   - Devices broadcast their presence via UDP
   - Discovery service automatically detects devices
   - If `require_pairing` is false, devices auto-connect
   - If `require_pairing` is true, only allowed devices connect

## Implementation Strategy

### Phase 1: Backend PIN Infrastructure

#### 1.1 PIN Generation Service
**Location**: `src-tauri/src/pairing/mod.rs`
```rust
pub struct PairingManager {
    current_pin: Arc<RwLock<PairingPin>>,
    pending_pairs: Arc<RwLock<HashMap<Uuid, PendingPair>>>,
    pin_refresh_handle: Option<JoinHandle<()>>,
}

pub struct PairingPin {
    code: String,
    generated_at: Instant,
    expires_at: Instant,
}

pub struct PendingPair {
    device_id: Uuid,
    device_info: DeviceInfo,
    initiated_at: Instant,
    pin_hash: String,
}
```

**Key Features**:
- Cryptographically secure 6-digit PIN generation using `ring` crate
- Auto-refresh every 120 seconds
- SHA-256 hashing for PIN verification
- Rate limiting (3 attempts per device per 5 minutes)
- Expiry management for pending pairing requests

#### 1.2 Enhanced Message Protocol
**Location**: `src-tauri/src/network/protocol.rs`
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MessageType {
    // ... existing types ...
    
    // New pairing messages
    PairingRequest {
        device_id: Uuid,
        device_name: String,
        public_key: Vec<u8>,
    },
    PairingChallenge {
        nonce: Vec<u8>,
        salt: Vec<u8>,
    },
    PairingResponse {
        pin_hash: String,
        signature: Vec<u8>,
    },
    PairingResult {
        success: bool,
        shared_secret: Option<Vec<u8>>,
        reason: Option<String>,
    },
}
```

#### 1.3 Secure Storage for Paired Devices
**Location**: `src-tauri/src/config/settings.rs`
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedDevice {
    pub id: Uuid,
    pub name: String,
    pub paired_at: u64,
    pub trust_level: TrustLevel,
    pub public_key: Vec<u8>,
    pub shared_secret: Vec<u8>,
    pub metadata: DeviceMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceMetadata {
    pub platform: String,
    pub app_version: String,
    pub last_connected: u64,
    pub total_transfers: u32,
}
```

### Phase 2: Frontend UI Implementation

#### 2.1 New Pairing Tab Component
**Location**: `src/components/PairingTab.tsx`
```typescript
interface PairingTabProps {
    currentPin: PairingPin;
    availableDevices: DeviceInfo[];
    onPairDevice: (deviceId: string, pin: string) => Promise<void>;
    onRefreshPin: () => void;
}

const PairingTab: React.FC<PairingTabProps> = ({...}) => {
    // PIN display with countdown timer
    // Available devices list with filters
    // Pairing modal with PIN input
    // Real-time pairing status updates
}
```

#### 2.2 Enhanced Navigation
**Location**: `src/components/Navigation.tsx`
```typescript
const tabs = [
    { id: 'devices', label: 'Devices', icon: Monitor },
    { id: 'pairing', label: 'Pairing', icon: Link2 },  // NEW
    { id: 'settings', label: 'Settings', icon: Settings },
    { id: 'info', label: 'Info', icon: Info },
];
```

#### 2.3 PIN Display Component
**Location**: `src/components/PinDisplay.tsx`
```typescript
interface PinDisplayProps {
    pin: string;
    expiresIn: number;
    onRefresh: () => void;
}

const PinDisplay: React.FC<PinDisplayProps> = ({...}) => {
    // Large, clear PIN display
    // Circular progress indicator for expiry
    // Manual refresh button
    // Copy to clipboard functionality
}
```

### Phase 3: Pairing Flow Implementation

#### 3.1 Device A (Initiator) Flow
1. User navigates to Pairing tab
2. Sees list of unpaired devices
3. Clicks "Pair" on Device B
4. Modal appears requesting Device B's PIN
5. User enters PIN from Device B
6. System validates PIN and establishes secure connection
7. Both devices add each other to paired list

#### 3.2 Device B (Receiver) Flow
1. Displays current PIN prominently
2. PIN auto-refreshes every 2 minutes
3. Receives pairing request from Device A
4. Validates PIN hash
5. Sends confirmation
6. Updates paired devices list

#### 3.3 Security Implementation
```rust
// Pairing verification flow
impl PairingManager {
    pub async fn verify_pairing_request(
        &self,
        device_id: Uuid,
        provided_pin: String,
        peer_public_key: &[u8],
    ) -> Result<Vec<u8>> {
        // 1. Verify PIN hasn't expired
        // 2. Check rate limiting
        // 3. Validate PIN hash
        // 4. Generate shared secret using ECDH
        // 5. Store paired device info
        // 6. Return shared secret
    }
}
```

### Phase 4: State Management Updates

#### 4.1 Backend State
```rust
// In FileshareDaemon
pub struct FileshareDaemon {
    // ... existing fields ...
    pub pairing_manager: Arc<PairingManager>,
}

// In PeerManager
impl PeerManager {
    fn should_connect_to_peer(&self, device_info: &DeviceInfo) -> bool {
        // Only connect to paired devices
        self.paired_devices.contains(&device_info.id)
    }
}
```

#### 4.2 Frontend State
```typescript
// In App.tsx
const [currentPin, setCurrentPin] = useState<PairingPin | null>(null);
const [pairedDevices, setPairedDevices] = useState<DeviceInfo[]>([]);
const [availableDevices, setAvailableDevices] = useState<DeviceInfo[]>([]);
const [pairingStatus, setPairingStatus] = useState<Map<string, PairingStatus>>();
```

### Phase 5: Tauri Commands

#### 5.1 New Commands
```rust
#[tauri::command]
async fn get_current_pin(state: State<'_, AppState>) -> Result<PairingPin, String>;

#[tauri::command]
async fn refresh_pin(state: State<'_, AppState>) -> Result<PairingPin, String>;

#[tauri::command]
async fn initiate_pairing(
    device_id: String,
    pin: String,
    state: State<'_, AppState>
) -> Result<(), String>;

#[tauri::command]
async fn get_pairing_status(
    device_id: String,
    state: State<'_, AppState>
) -> Result<PairingStatus, String>;

#[tauri::command]
async fn get_paired_devices(state: State<'_, AppState>) -> Result<Vec<DeviceInfo>, String>;

#[tauri::command]
async fn get_unpaired_devices(state: State<'_, AppState>) -> Result<Vec<DeviceInfo>, String>;
```

### Phase 6: Database/Storage

#### 6.1 Configuration Updates
```toml
# config.toml structure
[security]
require_pairing = true
pairing_enabled = true
pin_lifetime_seconds = 120
max_pin_attempts = 3
rate_limit_window_seconds = 300

[paired_devices]
# Array of paired device entries
```

#### 6.2 Persistent Storage
- Store paired devices in encrypted format
- Use OS keychain for sensitive data (shared secrets)
- Regular cleanup of expired pairing attempts

### Phase 7: Error Handling & Edge Cases

1. **Network Interruptions**
   - Graceful handling of connection drops during pairing
   - Automatic retry with exponential backoff
   - Clear error messages to user

2. **PIN Expiry**
   - Warning when PIN is about to expire (10 seconds)
   - Automatic refresh with smooth UI transition
   - Prevent pairing with expired PINs

3. **Concurrent Pairing**
   - Queue multiple pairing requests
   - Prevent race conditions with mutex locks
   - Clear status indicators for each device

4. **Security Failures**
   - Log security events
   - Temporary device blocking after failed attempts
   - Admin override capabilities

### Phase 8: Testing Strategy

1. **Unit Tests**
   - PIN generation and validation
   - Cryptographic operations
   - Rate limiting logic

2. **Integration Tests**
   - Full pairing flow
   - Multi-device scenarios
   - Network failure recovery

3. **Security Tests**
   - Brute force protection
   - Replay attack prevention
   - Man-in-the-middle protection

## Implementation Timeline

### Week 1: Backend Core
- [ ] Implement PairingManager
- [ ] Add PIN generation service
- [ ] Update message protocol
- [ ] Add pairing commands

### Week 2: Frontend UI
- [ ] Create Pairing tab
- [ ] Implement PIN display
- [ ] Add device filtering
- [ ] Build pairing modal

### Week 3: Integration
- [ ] Connect frontend to backend
- [ ] Implement full pairing flow
- [ ] Add error handling
- [ ] Update existing components

### Week 4: Polish & Testing
- [ ] Security audit
- [ ] Performance optimization
- [ ] Comprehensive testing
- [ ] Documentation update

## Security Considerations

1. **PIN Generation**: Use cryptographically secure random number generator
2. **PIN Transmission**: Never send PIN in plaintext; use hash comparison
3. **Replay Protection**: Include timestamp and nonce in pairing messages
4. **Rate Limiting**: Prevent brute force attacks
5. **Secure Storage**: Encrypt paired device information at rest
6. **Perfect Forward Secrecy**: Generate new keys for each session
7. **Audit Logging**: Log all pairing attempts and failures

## UI/UX Guidelines

1. **PIN Display**
   - Large, readable font (minimum 24px)
   - High contrast colors
   - Clear separation between digits (e.g., "123-456")
   - Visible countdown timer

2. **Device Lists**
   - Clear distinction between paired/unpaired
   - Device type icons
   - Signal strength indicators
   - Last seen timestamps

3. **Pairing Process**
   - Maximum 3 clicks to complete pairing
   - Clear progress indicators
   - Immediate feedback on success/failure
   - Helpful error messages

## Migration Strategy

1. **Backward Compatibility**
   - Support legacy auto-connect for transition period
   - Provide migration wizard for existing users
   - Allow opt-out for local networks

2. **Data Migration**
   - Convert existing allowed_devices to paired_devices
   - Generate default metadata for existing connections
   - Preserve user preferences

## Performance Considerations

1. **PIN Refresh**: Use background timer, not blocking operations
2. **Device Discovery**: Maintain efficient caching
3. **UI Updates**: Use React.memo and useCallback for optimization
4. **Network Operations**: Implement connection pooling

## Conclusion

This implementation plan provides a secure, user-friendly pairing mechanism that maintains the application's performance while significantly enhancing security. The phased approach allows for incremental development and testing, ensuring a stable release.