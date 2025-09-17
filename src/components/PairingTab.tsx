import React, { useState, useEffect, useCallback, useMemo, useRef } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import {
    Search,
    Monitor,
    Smartphone,
    Tablet,
    Key,
    Clock,
    Loader2,
    CheckCircle,
    XCircle,
    WifiOff,
} from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { useTheme } from '../context/ThemeContext';

// Types
interface UnpairedDevice {
    id: string;
    name: string;
    address: string;
    version: string;
    platform?: string;
    last_seen: number;
}

interface PairingError {
    DeviceOffline?: null;
    ConnectionFailed?: null;
    RequestTimeout?: null;
    ConfirmationTimeout?: null;
    UserRejected?: null;
    CryptoError?: null;
    ProtocolError?: string;
    Unknown?: string;
}

interface PairingSession {
    session_id: string;
    peer_device_id: string;
    peer_name: string;
    pin: string;
    state: 'Initiated' | 'DisplayingPin' | 'AwaitingApproval' | 'Confirmed' | 'Completed' | { Failed: PairingError } | 'Timeout';
    initiated_by_us: boolean;
    remaining_seconds: number;
}

interface PairingTabProps {
    onRefresh: () => void;
}

const PairingTab: React.FC<PairingTabProps> = ({ onRefresh }) => {
    const { theme } = useTheme();
    const [unpairedDevices, setUnpairedDevices] = useState<UnpairedDevice[]>([]);
    const [activeSessions, setActiveSessions] = useState<Map<string, PairingSession>>(new Map());
    const [isLoading, setIsLoading] = useState(true);
    const [searchTerm, setSearchTerm] = useState('');
    const [osFilter, setOsFilter] = useState<'all' | 'mac' | 'windows' | 'linux'>('all');
    const mountedRef = useRef(true);
    const [hoveredButton, setHoveredButton] = useState<string | null>(null);

    // Load unpaired devices and active sessions
    const loadPairingData = async (showLoading = true) => {
        if (!mountedRef.current) return;

        try {
            if (showLoading) {
                setIsLoading(true);
            }

            // Get all discovered devices
            const allDevices = await invoke<any[]>('get_discovered_devices');

            // Get paired devices to filter out
            const pairedDevices = await invoke<any[]>('get_paired_devices');
            const pairedIds = new Set(pairedDevices.map(d => d.device_id));

            // Filter to only unpaired devices
            const unpaired = allDevices
                .filter(device => !pairedIds.has(device.id))
                .map(device => ({
                    id: device.id,
                    name: device.name,
                    address: device.address,
                    version: device.version,
                    platform: device.platform,
                    last_seen: device.last_seen
                }));

            // Get active pairing sessions
            const sessions = await invoke<PairingSession[]>('get_active_pairing_sessions');

            // Create a map of device ID to session
            const sessionMap = new Map<string, PairingSession>();
            sessions.forEach(session => {
                sessionMap.set(session.peer_device_id, session);
            });

            setUnpairedDevices(unpaired);
            setActiveSessions(sessionMap);

        } catch (err) {
            console.error('Failed to load pairing data:', err);
        } finally {
            if (showLoading) {
                setIsLoading(false);
            }
        }
    };

    // Filter devices based on search and OS filter
    const filteredDevices = useMemo(() => {
        let filtered = unpairedDevices;

        // Apply search filter
        if (searchTerm) {
            filtered = filtered.filter(device =>
                device.name.toLowerCase().includes(searchTerm.toLowerCase()) ||
                device.address.includes(searchTerm)
            );
        }

        // Apply OS filter
        if (osFilter !== 'all') {
            filtered = filtered.filter(device => {
                const platform = device.platform?.toLowerCase();
                switch (osFilter) {
                    case 'mac':
                        return platform === 'macos' || platform === 'darwin';
                    case 'windows':
                        return platform === 'windows';
                    case 'linux':
                        return platform === 'linux';
                    default:
                        return true;
                }
            });
        }

        return filtered;
    }, [unpairedDevices, searchTerm, osFilter]);

    // Initialize data and set up refresh interval
    useEffect(() => {
        mountedRef.current = true;
        loadPairingData(true);

        const refreshInterval = 2000;
        const interval = setInterval(() => loadPairingData(false), refreshInterval);

        return () => {
            mountedRef.current = false;
            clearInterval(interval);
        };
    }, []);

    // Initiate pairing with a device
    const handlePairDevice = useCallback(async (deviceId: string) => {
        try {
            await invoke('initiate_pairing', { deviceId });
            loadPairingData(false);
        } catch (err) {
            console.error('Failed to initiate pairing:', err);
        }
    }, []);

    // Confirm pairing
    const handleConfirmPairing = useCallback(async (sessionId: string) => {
        try {
            await invoke('confirm_pairing', { sessionId });
            loadPairingData(false);
        } catch (err) {
            console.error('Failed to confirm pairing:', err);
        }
    }, []);

    // Reject pairing
    const handleRejectPairing = useCallback(async (sessionId: string) => {
        try {
            await invoke('reject_pairing', {
                sessionId,
                reason: 'User rejected'
            });
            loadPairingData(false);
        } catch (err) {
            console.error('Failed to reject pairing:', err);
        }
    }, []);

    // Get device icon
    const getDeviceIcon = useCallback((platform?: string) => {
        const platform_lower = platform?.toLowerCase();
        if (platform_lower === 'android' || platform_lower === 'ios') {
            return <Smartphone className="w-4 h-4" />;
        } else if (platform_lower === 'windows' || platform_lower === 'macos' || platform_lower === 'darwin' || platform_lower === 'linux') {
            return <Monitor className="w-4 h-4" />;
        }
        return <Tablet className="w-4 h-4" />;
    }, []);

    // Get pairing button content based on session state
    const getPairingButton = (device: UnpairedDevice, session?: PairingSession) => {
        if (!session) {
            const buttonId = `pair-${device.id}`;
            const isHovered = hoveredButton === buttonId;
            return (
                <button
                    onClick={() => handlePairDevice(device.id)}
                    onMouseEnter={() => setHoveredButton(buttonId)}
                    onMouseLeave={() => setHoveredButton(null)}
                    className="px-3 py-1 rounded-full text-xs font-medium transition-colors"
                    style={{
                        backgroundColor: isHovered ? theme.colors.hover : theme.colors.backgroundTertiary,
                        color: theme.colors.text
                    }}
                >
                    Pair
                </button>
            );
        }

        // Handle different session states
        const state = session.state;
        if (typeof state === 'object' && 'Failed' in state) {
            const buttonId = `retry-${device.id}`;
            const isHovered = hoveredButton === buttonId;
            return (
                <button
                    onClick={() => handlePairDevice(device.id)}
                    onMouseEnter={() => setHoveredButton(buttonId)}
                    onMouseLeave={() => setHoveredButton(null)}
                    className="px-3 py-1 rounded-full text-xs font-medium transition-colors flex items-center gap-1"
                    style={{
                        backgroundColor: isHovered ? '#FF6666' : theme.colors.error,
                        color: theme.colors.text
                    }}
                >
                    <XCircle className="w-3 h-3" />
                    Retry
                </button>
            );
        }

        switch (state) {
            case 'Initiated':
            case 'Confirmed':
                return (
                    <button
                        disabled
                        className="px-3 py-1 rounded-full text-xs font-medium flex items-center gap-1"
                        style={{
                            backgroundColor: theme.colors.backgroundTertiary,
                            color: theme.colors.textSecondary
                        }}
                    >
                        <Loader2 className="w-3 h-3 animate-spin" />
                        Pairing
                    </button>
                );
            case 'DisplayingPin':
                return (
                    <button
                        disabled
                        className="px-3 py-1 rounded-full text-xs font-medium flex items-center gap-1"
                        style={{
                            backgroundColor: theme.colors.accent2,
                            color: theme.colors.text
                        }}
                    >
                        <Key className="w-3 h-3" />
                        Pairing
                    </button>
                );
            case 'AwaitingApproval':
                const acceptButtonId = `accept-${session.session_id}`;
                const rejectButtonId = `reject-${session.session_id}`;
                const isAcceptHovered = hoveredButton === acceptButtonId;
                const isRejectHovered = hoveredButton === rejectButtonId;
                return (
                    <div className="flex gap-2">
                        <button
                            onClick={() => handleConfirmPairing(session.session_id)}
                            onMouseEnter={() => setHoveredButton(acceptButtonId)}
                            onMouseLeave={() => setHoveredButton(null)}
                            className="px-3 py-1 rounded-full text-xs font-medium transition-colors"
                            style={{
                                backgroundColor: isAcceptHovered ? '#00FF9E' : theme.colors.success,
                                color: theme.colors.text
                            }}
                        >
                            Accept
                        </button>
                        <button
                            onClick={() => handleRejectPairing(session.session_id)}
                            onMouseEnter={() => setHoveredButton(rejectButtonId)}
                            onMouseLeave={() => setHoveredButton(null)}
                            className="px-3 py-1 rounded-full text-xs font-medium transition-colors"
                            style={{
                                backgroundColor: isRejectHovered ? theme.colors.hover : theme.colors.backgroundTertiary,
                                color: theme.colors.text
                            }}
                        >
                            Reject
                        </button>
                    </div>
                );
            case 'Completed':
                return (
                    <button
                        disabled
                        className="px-3 py-1 rounded-full text-xs font-medium flex items-center gap-1"
                        style={{
                            backgroundColor: theme.colors.success,
                            color: theme.colors.text
                        }}
                    >
                        <CheckCircle className="w-3 h-3" />
                        Paired
                    </button>
                );
            default:
                const defaultButtonId = `default-pair-${device.id}`;
                const isDefaultHovered = hoveredButton === defaultButtonId;
                return (
                    <button
                        onClick={() => handlePairDevice(device.id)}
                        onMouseEnter={() => setHoveredButton(defaultButtonId)}
                        onMouseLeave={() => setHoveredButton(null)}
                        className="px-3 py-1 rounded-full text-xs font-medium transition-colors"
                        style={{
                            backgroundColor: isDefaultHovered ? theme.colors.hover : theme.colors.backgroundTertiary,
                            color: theme.colors.text
                        }}
                    >
                        Pair
                    </button>
                );
        }
    };

    return (
        <div className="space-y-4">
            <h2 className="text-lg font-medium" style={{ color: theme.colors.text }}>Available Devices</h2>

            {/* Search Bar */}
            <div className="relative">
                <Search className="absolute left-3 top-1/2 transform -translate-y-1/2 w-4 h-4" style={{ color: theme.colors.textSecondary }} />
                <input
                    type="text"
                    placeholder="Search Devices..."
                    value={searchTerm}
                    onChange={(e) => setSearchTerm(e.target.value)}
                    className="w-full pl-10 pr-4 py-2 rounded-lg text-sm focus:outline-none focus:ring-2 focus:border-transparent placeholder-color"
                    style={{
                        backgroundColor: theme.colors.backgroundSecondary + '80',
                        borderColor: theme.colors.border,
                        borderWidth: '1px',
                        color: theme.colors.text,
                        ['--placeholder-color' as any]: theme.colors.textTertiary
                    }}
                />
            </div>

            {/* OS Filter Buttons */}
            <div className="flex items-center gap-2">
                {[{ key: 'all', label: 'All' }, { key: 'mac', label: 'Mac' }, { key: 'windows', label: 'Windows' }, { key: 'linux', label: 'Linux' }].map(({ key, label }) => {
                    const isActive = osFilter === key;
                    const buttonId = `filter-${key}`;
                    const isHovered = hoveredButton === buttonId;
                    return (
                        <button
                            key={key}
                            onClick={() => setOsFilter(key as any)}
                            onMouseEnter={() => setHoveredButton(buttonId)}
                            onMouseLeave={() => setHoveredButton(null)}
                            className="px-4 py-1.5 rounded-full text-sm font-medium transition-colors"
                            style={{
                                backgroundColor: isActive
                                    ? theme.colors.accent2
                                    : isHovered
                                        ? theme.colors.backgroundTertiary
                                        : theme.colors.backgroundSecondary,
                                color: isActive
                                    ? theme.colors.text
                                    : isHovered
                                        ? theme.colors.text
                                        : theme.colors.textSecondary
                            }}
                        >
                            {label}
                        </button>
                    );
                })}
            </div>

            {/* Device List */}
            {isLoading ? (
                <div className="space-y-2">
                    {[...Array(3)].map((_, i) => (
                        <div
                            key={i}
                            className="rounded-lg h-12 animate-pulse"
                            style={{ backgroundColor: theme.colors.backgroundSecondary + '50' }}
                        />
                    ))}
                </div>
            ) : filteredDevices.length === 0 ? (
                <motion.div
                    initial={{ opacity: 0, scale: 0.8 }}
                    animate={{ opacity: 1, scale: 1 }}
                    className="text-center py-6 mt-12"
                >
                    <WifiOff className="w-12 h-12 mx-auto mb-3" style={{ color: theme.colors.textSecondary }} />
                    <p className="text-sm" style={{ color: theme.colors.textSecondary }}>
                        {searchTerm
                            ? 'No devices match your search'
                            : 'No unpaired devices found'
                        }
                    </p>
                    <p className="text-xs mt-1" style={{ color: theme.colors.textTertiary }}>
                        {searchTerm
                            ? 'Try adjusting your search'
                            : 'Make sure other devices are running Yeet'
                        }
                    </p>
                    <motion.button
                        whileHover={{ scale: 1.05 }}
                        whileTap={{ scale: 0.95 }}
                        onClick={onRefresh}
                        className="mt-3 px-3 py-1 text-xs rounded transition-colors"
                        style={{
                            backgroundColor: theme.colors.accent2 + '20',
                            color: theme.colors.accent2
                        }}
                        onMouseEnter={(e) => {
                            e.currentTarget.style.backgroundColor = theme.colors.accent2 + '30';
                        }}
                        onMouseLeave={(e) => {
                            e.currentTarget.style.backgroundColor = theme.colors.accent2 + '20';
                        }}
                    >
                        Refresh
                    </motion.button>
                </motion.div>
            ) : (
                <div className="space-y-2">
                    <motion.div
                        initial={{ opacity: 0 }}
                        animate={{ opacity: 1 }}
                        className="text-xs mb-2"
                        style={{ color: theme.colors.textTertiary }}
                    >
                        Showing {filteredDevices.length} device{filteredDevices.length !== 1 ? 's' : ''}
                    </motion.div>

                    {filteredDevices.map((device) => {
                        const session = activeSessions.get(device.id);
                        const showPin = session?.state === 'DisplayingPin' && session.initiated_by_us;
                        const showApprovalPin = session?.state === 'AwaitingApproval' && !session.initiated_by_us;

                        return (
                            <div key={device.id} className="space-y-2">
                                <motion.div
                                    className="rounded-lg px-4 py-3 transition-colors border"
                                    style={{
                                        backgroundColor: theme.colors.backgroundSecondary + '50',
                                        borderColor: theme.colors.border
                                    }}
                                    onMouseEnter={(e) => {
                                        e.currentTarget.style.backgroundColor = theme.colors.backgroundSecondary + '80';
                                    }}
                                    onMouseLeave={(e) => {
                                        e.currentTarget.style.backgroundColor = theme.colors.backgroundSecondary + '50';
                                    }}
                                >
                                    <div className="flex items-center justify-between">
                                        <div className="flex items-center gap-3">
                                            <div style={{ color: theme.colors.textSecondary }}>
                                                {getDeviceIcon(device.platform)}
                                            </div>
                                            <span className="text-sm font-medium" style={{ color: theme.colors.text }}>{device.name}</span>
                                        </div>
                                        {getPairingButton(device, session)}
                                    </div>
                                </motion.div>

                                {/* PIN Display for Initiator */}
                                <AnimatePresence>
                                    {showPin && (
                                        <motion.div
                                            initial={{ opacity: 0, height: 0 }}
                                            animate={{ opacity: 1, height: 'auto' }}
                                            exit={{ opacity: 0, height: 0 }}
                                            className="rounded-lg p-4 ml-4 border"
                                            style={{
                                                backgroundColor: theme.colors.backgroundSecondary + '80',
                                                borderColor: theme.colors.border
                                            }}
                                        >
                                            <div className="flex items-start gap-3">
                                                <Key className="w-4 h-4 mt-1" style={{ color: theme.colors.textSecondary }} />
                                                <div className="flex-1">
                                                    <h4 className="text-sm font-medium mb-1" style={{ color: theme.colors.text }}>Your Pairing PIN</h4>
                                                    <p className="text-xs mb-3" style={{ color: theme.colors.textTertiary }}>
                                                        Share this PIN with devices that want to pair with you
                                                    </p>
                                                    <div className="text-2xl font-mono font-bold mb-2" style={{ color: theme.colors.text }}>
                                                        {session.pin}
                                                    </div>
                                                    {session.remaining_seconds > 0 && (
                                                        <div className="flex items-center gap-1 text-xs" style={{ color: theme.colors.textTertiary }}>
                                                            <Clock className="w-3 h-3" />
                                                            <span>
                                                                {Math.floor(session.remaining_seconds / 60)}:
                                                                {(session.remaining_seconds % 60).toString().padStart(2, '0')} remaining
                                                            </span>
                                                        </div>
                                                    )}
                                                </div>
                                            </div>
                                        </motion.div>
                                    )}

                                    {/* PIN Display for Approval */}
                                    {showApprovalPin && (
                                        <motion.div
                                            initial={{ opacity: 0, height: 0 }}
                                            animate={{ opacity: 1, height: 'auto' }}
                                            exit={{ opacity: 0, height: 0 }}
                                            className="rounded-lg p-4 ml-4 border"
                                            style={{
                                                backgroundColor: theme.colors.warning + '20',
                                                borderColor: theme.colors.warning + '70'
                                            }}
                                        >
                                            <div className="flex items-start gap-3">
                                                <Key className="w-4 h-4 mt-1" style={{ color: theme.colors.warning }} />
                                                <div className="flex-1">
                                                    <h4 className="text-sm font-medium mb-1" style={{ color: theme.colors.text }}>Verify PIN</h4>
                                                    <p className="text-xs mb-3" style={{ color: theme.colors.textTertiary }}>
                                                        Make sure this matches the PIN shown on {session.peer_name}
                                                    </p>
                                                    <div className="text-2xl font-mono font-bold mb-2" style={{ color: theme.colors.warning }}>
                                                        {session.pin}
                                                    </div>
                                                </div>
                                            </div>
                                        </motion.div>
                                    )}
                                </AnimatePresence>
                            </div>
                        );
                    })}
                </div>
            )}
        </div>
    );
};

export default PairingTab;