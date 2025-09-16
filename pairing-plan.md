# Bidirectional PIN-Based Pairing Implementation Plan

## Overview
Implement a secure bidirectional PIN-based pairing system where both devices must confirm the same PIN before establishing trust. This ensures both parties explicitly consent to the pairing and prevents unauthorized access.

## Pairing Flow Diagram

```
┌─────────────┐                                    ┌─────────────┐
│  Device A   │                                    │  Device B   │
└─────────────┘                                    └─────────────┘
      │                                                    │
      │  1. Discovers Device B via UDP broadcast          │
      ├───────────────────────────────────────────────────┤
      │                                                    │
      │  2. User clicks "Pair" in PAIRING tab            │
      ├─────────────────────►                            │
      │   PairingRequest                                  │
      │   {device_id, name,                              │
      │    public_key}                                   │
      │                                                   │
      │                     ◄─────────────────────────────┤
      │                      PairingChallenge             │
      │                      {device_id, name,            │
      │                       public_key, PIN}            │
      │                                                    │
      │  3. Both devices display same PIN                 │
      ├───────────────────────────────────────────────────┤
      │     Device A shows:  │  Device B shows:           │
      │     "Verify: 1234"   │  "Verify: 1234"           │
      │                                                    │
      │  4. Users confirm PIN matches on both devices     │
      ├───────────────────────────────────────────────────┤
      │                                                    │
      │  5. User clicks "Confirm"                         │
      ├─────────────────────►                             │
      │   PairingConfirm                                  │
      │   {signed_challenge}                              │
      │                                                    │
      │                     ◄─────────────────────────────┤
      │                      PairingComplete               │
      │                      {signed_acknowledgment}       │
      │                                                    │
      │  6. Both devices now paired and trusted           │
      └───────────────────────────────────────────────────┘
```

## Technical Architecture

### 1. Data Structures

#### Rust Backend (`src-tauri/src/pairing/`)

```rust
// pairing/mod.rs
pub struct PairingManager {
    active_pairing_sessions: HashMap<Uuid, PairingSession>,
    paired_devices: HashMap<Uuid, PairedDevice>,
    device_keypair: DeviceKeypair,
}

pub struct PairingSession {
    peer_device_id: Uuid,
    peer_name: String,
    peer_public_key: PublicKey,
    pin: String,
    state: PairingState,
    initiated_by_us: bool,
    created_at: Instant,
    expires_at: Instant,
}

pub enum PairingState {
    Initiated,        // Request sent, waiting for challenge
    Challenging,      // PIN displayed, waiting for user confirmation
    AwaitingConfirm,  // User confirmed, waiting for peer
    Confirmed,        // Both confirmed, completing pairing
    Completed,        // Successfully paired
    Failed(String),   // Pairing failed
}

pub struct PairedDevice {
    device_id: Uuid,
    name: String,
    public_key: PublicKey,
    paired_at: SystemTime,
    last_seen: SystemTime,
    trust_level: TrustLevel,
    metadata: DeviceMetadata,
}

pub struct DeviceKeypair {
    public_key: PublicKey,
    private_key: PrivateKey,
    certificate: Option<Certificate>,
}
```

#### Updated Config Structure

```rust
// config/settings.rs - additions
pub struct PairedDevice {
    pub id: Uuid,
    pub name: String,
    pub public_key_pem: String,
    pub paired_at: u64,
    pub last_seen: u64,
    pub auto_accept_files: bool,
}

pub struct SecuritySettings {
    pub require_pairing: bool,  // Set to true
    pub paired_devices: Vec<PairedDevice>,
    pub device_private_key: Option<String>,  // PEM encoded
    pub device_public_key: Option<String>,   // PEM encoded
    pub auto_accept_from_paired: bool,
    pub pairing_timeout_seconds: u64,  // Default: 300 (5 mins)
}
```

### 2. Protocol Messages

```rust
// network/protocol.rs - additions
pub enum Message {
    // Existing messages...
    
    // Pairing messages
    PairingRequest {
        device_id: Uuid,
        device_name: String,
        public_key: Vec<u8>,
        timestamp: u64,
    },
    
    PairingChallenge {
        device_id: Uuid,
        device_name: String,
        public_key: Vec<u8>,
        pin: String,
        session_id: Uuid,
        timestamp: u64,
    },
    
    PairingConfirm {
        session_id: Uuid,
        signed_challenge: Vec<u8>,  // PIN signed with private key
        timestamp: u64,
    },
    
    PairingComplete {
        session_id: Uuid,
        signed_acknowledgment: Vec<u8>,
        device_metadata: DeviceMetadata,
    },
    
    PairingReject {
        session_id: Uuid,
        reason: String,
    },
    
    PairingTimeout {
        session_id: Uuid,
    },
}
```

### 3. Frontend Components

#### New Components

1. **PairingTab.tsx**
   - Shows discovered unpaired devices
   - Initiates pairing requests
   - Displays pairing dialogs

2. **PairingDialog.tsx**
   - Shows PIN for verification
   - Confirm/Cancel buttons
   - Timeout countdown
   - Loading states

3. **PairedDevicesList.tsx**
   - Shows only paired devices in DEVICES tab
   - Online/offline status
   - Last seen timestamp
   - Unpair action

#### Updated Components

1. **Navigation.tsx**
   - Add PAIRING tab
   - Show badge with unpaired device count

2. **DevicesList.tsx**
   - Filter to show only paired devices
   - Add pairing status indicator

3. **App.tsx**
   - Add pairing state management
   - Handle pairing events from backend

## Implementation Steps

### Phase 1: Backend Infrastructure (Rust)

#### Step 1.1: Create Pairing Module
```
src-tauri/src/pairing/
├── mod.rs           # Public interface
├── manager.rs       # PairingManager implementation
├── crypto.rs        # Keypair generation and signing
├── session.rs       # Pairing session management
└── pin.rs          # PIN generation and validation
```

#### Step 1.2: Cryptographic Implementation
- Generate ED25519 keypair on first launch
- Store keypair in secure system keychain (or encrypted file)
- Implement message signing/verification
- Generate cryptographically secure 4-digit PINs

#### Step 1.3: Update Protocol
- Add pairing messages to QUIC protocol
- Implement message handlers in PeerManager
- Add pairing state to peer connections

### Phase 2: State Management

#### Step 2.1: Persistence
- Extend Settings struct with paired devices
- Save paired device public keys
- Implement secure storage for private key
- Migration for existing configs

#### Step 2.2: Runtime State
- Track active pairing sessions
- Implement session timeouts (5 minutes)
- Clean up expired sessions
- Prevent duplicate pairing attempts

### Phase 3: Frontend UI

#### Step 3.1: PAIRING Tab
- Create new tab in navigation
- List discovered unpaired devices
- "Pair" button for each device
- Show device metadata (name, IP, platform)

#### Step 3.2: Pairing Dialog
- Modal overlay during pairing
- Large PIN display
- Confirm/Cancel buttons
- Progress indicators
- Error messages

#### Step 3.3: Update DEVICES Tab
- Show only paired devices
- Add "Forget Device" action
- Connection status indicators
- Last activity timestamp

### Phase 4: Integration

#### Step 4.1: Enforce Pairing
- Update PeerManager to check pairing status
- Reject file transfers from unpaired devices
- Update discovery to mark paired devices
- Add pairing check to clipboard operations

#### Step 4.2: Tauri Commands
```rust
#[tauri::command]
async fn initiate_pairing(device_id: String) -> Result<PairingSession>

#[tauri::command]
async fn confirm_pairing(session_id: String) -> Result<()>

#[tauri::command]
async fn reject_pairing(session_id: String) -> Result<()>

#[tauri::command]
async fn unpair_device(device_id: String) -> Result<()>

#[tauri::command]
async fn get_paired_devices() -> Result<Vec<PairedDevice>>

#[tauri::command]
async fn get_active_pairing_sessions() -> Result<Vec<PairingSession>>
```

### Phase 5: Security Hardening

#### Step 5.1: PIN Security
- Use constant-time comparison for PIN verification
- Rate limit pairing attempts (max 3 per minute)
- Implement exponential backoff for failed attempts
- Clear PIN from memory after use

#### Step 5.2: Message Security
- Sign all pairing messages
- Verify signatures before processing
- Implement replay attack prevention (timestamps)
- Add nonce to prevent message replay

## UI/UX Flow

### Initiating Pairing (Device A)
1. User opens app, goes to PAIRING tab
2. Sees list of discovered devices
3. Clicks "Pair" next to Device B
4. Dialog appears: "Requesting pairing..."
5. PIN appears: "Verify this PIN: 1234"
6. User verifies PIN matches on Device B
7. User clicks "Confirm"
8. Success message: "Device paired successfully!"
9. Device moves from PAIRING to DEVICES tab

### Receiving Pairing (Device B)
1. Notification: "Device A wants to pair"
2. Dialog automatically opens
3. Shows PIN: "Verify this PIN: 1234"
4. User verifies PIN matches on Device A
5. User clicks "Confirm"
6. Success message: "Device paired successfully!"
7. Device appears in DEVICES tab

### Edge Cases
- **Timeout**: After 5 minutes, cancel pairing
- **User cancels**: Send PairingReject message
- **Network issues**: Show appropriate error
- **Already paired**: Show info message
- **Multiple requests**: Queue or reject

## Testing Strategy

### Unit Tests
- PIN generation uniqueness
- Cryptographic operations
- Session timeout logic
- Message serialization

### Integration Tests
- Full pairing flow
- Timeout scenarios
- Rejection handling
- Persistence across restarts

### Manual Testing
1. Pair two devices successfully
2. Verify file transfer only works after pairing
3. Test rejection flow
4. Test timeout scenarios
5. Test unpair and re-pair
6. Test with 3+ devices

## Migration Plan

### For Existing Users
1. On update, set `require_pairing = false` initially
2. Show notification about new pairing feature
3. Allow gradual migration (optional at first)
4. In future version, make pairing mandatory

### Configuration Migration
```rust
// Detect old config format
if !settings.security.device_private_key.is_some() {
    // Generate keypair
    let keypair = generate_device_keypair();
    settings.security.device_private_key = Some(keypair.private);
    settings.security.device_public_key = Some(keypair.public);
    settings.save()?;
}
```

## Success Criteria

1. ✅ Devices cannot transfer files without pairing
2. ✅ PIN is displayed on both devices
3. ✅ Both users must confirm PIN
4. ✅ Pairing persists across app restarts
5. ✅ Can unpair devices
6. ✅ Paired devices auto-reconnect
7. ✅ Clear error messages for failures
8. ✅ Secure against MITM attacks
9. ✅ Timeout after 5 minutes
10. ✅ UI clearly shows paired vs unpaired

## Timeline Estimate

- **Phase 1 (Backend)**: 4-6 hours
- **Phase 2 (State)**: 2-3 hours  
- **Phase 3 (Frontend)**: 4-5 hours
- **Phase 4 (Integration)**: 3-4 hours
- **Phase 5 (Security)**: 2-3 hours
- **Testing**: 2-3 hours

**Total: ~20 hours of development**

## Next Steps

1. Create pairing module structure
2. Implement keypair generation
3. Add pairing messages to protocol
4. Create PairingTab component
5. Integrate with PeerManager
6. Test with two devices

Let's start implementing this plan!