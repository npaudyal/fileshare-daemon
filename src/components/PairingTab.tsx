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
    const [unpairedDevices, setUnpairedDevices] = useState<UnpairedDevice[]>([]);
    const [activeSessions, setActiveSessions] = useState<Map<string, PairingSession>>(new Map());
    const [isLoading, setIsLoading] = useState(true);
    const [searchTerm, setSearchTerm] = useState('');
    const [osFilter, setOsFilter] = useState<'all' | 'mac' | 'windows' | 'linux'>('all');
    const mountedRef = useRef(true);

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
            return (
                <button
                    onClick={() => handlePairDevice(device.id)}
                    className="px-3 py-1 bg-gray-700 hover:bg-gray-600 text-white rounded-full text-xs font-medium transition-colors"
                >
                    Pair
                </button>
            );
        }

        // Handle different session states
        const state = session.state;
        if (typeof state === 'object' && 'Failed' in state) {
            return (
                <button
                    onClick={() => handlePairDevice(device.id)}
                    className="px-3 py-1 bg-red-600 hover:bg-red-700 text-white rounded-full text-xs font-medium transition-colors flex items-center gap-1"
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
                        className="px-3 py-1 bg-gray-700 text-gray-400 rounded-full text-xs font-medium flex items-center gap-1"
                    >
                        <Loader2 className="w-3 h-3 animate-spin" />
                        Pairing
                    </button>
                );
            case 'DisplayingPin':
                return (
                    <button
                        disabled
                        className="px-3 py-1 bg-blue-600 text-white rounded-full text-xs font-medium flex items-center gap-1"
                    >
                        <Key className="w-3 h-3" />
                        Pairing
                    </button>
                );
            case 'AwaitingApproval':
                return (
                    <div className="flex gap-2">
                        <button
                            onClick={() => handleConfirmPairing(session.session_id)}
                            className="px-3 py-1 bg-green-600 hover:bg-green-700 text-white rounded-full text-xs font-medium transition-colors"
                        >
                            Accept
                        </button>
                        <button
                            onClick={() => handleRejectPairing(session.session_id)}
                            className="px-3 py-1 bg-gray-700 hover:bg-gray-600 text-white rounded-full text-xs font-medium transition-colors"
                        >
                            Reject
                        </button>
                    </div>
                );
            case 'Completed':
                return (
                    <button
                        disabled
                        className="px-3 py-1 bg-green-600 text-white rounded-full text-xs font-medium flex items-center gap-1"
                    >
                        <CheckCircle className="w-3 h-3" />
                        Paired
                    </button>
                );
            default:
                return (
                    <button
                        onClick={() => handlePairDevice(device.id)}
                        className="px-3 py-1 bg-gray-700 hover:bg-gray-600 text-white rounded-full text-xs font-medium transition-colors"
                    >
                        Pair
                    </button>
                );
        }
    };

    return (
        <div className="space-y-4">
            <h2 className="text-lg font-medium text-white">Available Devices</h2>

            {/* Search Bar */}
            <div className="relative">
                <Search className="absolute left-3 top-1/2 transform -translate-y-1/2 text-gray-400 w-4 h-4" />
                <input
                    type="text"
                    placeholder="Search Devices..."
                    value={searchTerm}
                    onChange={(e) => setSearchTerm(e.target.value)}
                    className="w-full pl-10 pr-4 py-2 bg-gray-800/50 border border-gray-700 rounded-lg text-white placeholder-gray-500 focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent text-sm"
                />
            </div>

            {/* OS Filter Buttons */}
            <div className="flex items-center gap-2">
                <button
                    onClick={() => setOsFilter('all')}
                    className={`px-4 py-1.5 rounded-full text-sm font-medium transition-colors ${osFilter === 'all'
                        ? 'bg-blue-600 text-white'
                        : 'bg-gray-800 text-gray-400 hover:text-white hover:bg-gray-700'
                        }`}
                >
                    All
                </button>
                <button
                    onClick={() => setOsFilter('mac')}
                    className={`px-4 py-1.5 rounded-full text-sm font-medium transition-colors ${osFilter === 'mac'
                        ? 'bg-blue-600 text-white'
                        : 'bg-gray-800 text-gray-400 hover:text-white hover:bg-gray-700'
                        }`}
                >
                    Mac
                </button>
                <button
                    onClick={() => setOsFilter('windows')}
                    className={`px-4 py-1.5 rounded-full text-sm font-medium transition-colors ${osFilter === 'windows'
                        ? 'bg-blue-600 text-white'
                        : 'bg-gray-800 text-gray-400 hover:text-white hover:bg-gray-700'
                        }`}
                >
                    Windows
                </button>
                <button
                    onClick={() => setOsFilter('linux')}
                    className={`px-4 py-1.5 rounded-full text-sm font-medium transition-colors ${osFilter === 'linux'
                        ? 'bg-blue-600 text-white'
                        : 'bg-gray-800 text-gray-400 hover:text-white hover:bg-gray-700'
                        }`}
                >
                    Linux
                </button>
            </div>

            {/* Device List */}
            {isLoading ? (
                <div className="space-y-2">
                    {[...Array(3)].map((_, i) => (
                        <div key={i} className="bg-gray-800/30 rounded-lg h-12 animate-pulse" />
                    ))}
                </div>
            ) : filteredDevices.length === 0 ? (
                <motion.div
                    initial={{ opacity: 0, scale: 0.8 }}
                    animate={{ opacity: 1, scale: 1 }}
                    className="text-center py-6 mt-12"
                >
                    <WifiOff className="w-12 h-12 text-gray-400 mx-auto mb-3" />
                    <p className="text-gray-400 text-sm">
                        {searchTerm
                            ? 'No devices match your search'
                            : 'No unpaired devices found'
                        }
                    </p>
                    <p className="text-gray-500 text-xs mt-1">
                        {searchTerm
                            ? 'Try adjusting your search'
                            : 'Make sure other devices are running Yeet'
                        }
                    </p>
                    <motion.button
                        whileHover={{ scale: 1.05 }}
                        whileTap={{ scale: 0.95 }}
                        onClick={onRefresh}
                        className="mt-3 px-3 py-1 text-xs bg-blue-500/20 text-blue-300 rounded hover:bg-blue-500/30 transition-colors"
                    >
                        Refresh
                    </motion.button>
                </motion.div>
            ) : (
                <div className="space-y-2">
                    <motion.div
                        initial={{ opacity: 0 }}
                        animate={{ opacity: 1 }}
                        className="text-xs text-gray-500 mb-2"
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
                                    className="bg-gray-800/30 border border-gray-700 rounded-lg px-4 py-3 hover:bg-gray-800/50 transition-colors"
                                >
                                    <div className="flex items-center justify-between">
                                        <div className="flex items-center gap-3">
                                            <div className="text-gray-400">
                                                {getDeviceIcon(device.platform)}
                                            </div>
                                            <span className="text-white text-sm font-medium">{device.name}</span>
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
                                            className="bg-gray-800/50 border border-gray-700 rounded-lg p-4 ml-4"
                                        >
                                            <div className="flex items-start gap-3">
                                                <Key className="w-4 h-4 text-gray-400 mt-1" />
                                                <div className="flex-1">
                                                    <h4 className="text-white text-sm font-medium mb-1">Your Pairing PIN</h4>
                                                    <p className="text-xs text-gray-500 mb-3">
                                                        Share this PIN with devices that want to pair with you
                                                    </p>
                                                    <div className="text-2xl font-mono font-bold text-white mb-2">
                                                        {session.pin}
                                                    </div>
                                                    {session.remaining_seconds > 0 && (
                                                        <div className="flex items-center gap-1 text-xs text-gray-500">
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
                                            className="bg-yellow-900/20 border border-yellow-700 rounded-lg p-4 ml-4"
                                        >
                                            <div className="flex items-start gap-3">
                                                <Key className="w-4 h-4 text-yellow-400 mt-1" />
                                                <div className="flex-1">
                                                    <h4 className="text-white text-sm font-medium mb-1">Verify PIN</h4>
                                                    <p className="text-xs text-gray-500 mb-3">
                                                        Make sure this matches the PIN shown on {session.peer_name}
                                                    </p>
                                                    <div className="text-2xl font-mono font-bold text-yellow-400 mb-2">
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