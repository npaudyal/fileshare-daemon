import React, { useState, useEffect } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import {
    Wifi,
    Shield,
    Clock,
    Smartphone,
    Monitor,
    Tablet,
    AlertCircle,
    CheckCircle,
    XCircle,
    RefreshCw,
    Search,
    Filter,
    Apple,
    Chrome
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

interface PairingSession {
    session_id: string;
    peer_device_id: string;
    peer_name: string;
    pin: string;
    state: 'Initiated' | 'Challenging' | 'AwaitingConfirm' | 'Confirmed' | 'Completed' | { Failed: string } | 'Timeout';
    initiated_by_us: boolean;
    remaining_seconds: number;
}

interface PairingTabProps {
    onRefresh: () => void;
}

const PairingTab: React.FC<PairingTabProps> = ({ onRefresh }) => {
    const [unpairedDevices, setUnpairedDevices] = useState<UnpairedDevice[]>([]);
    const [activeSessions, setActiveSessions] = useState<PairingSession[]>([]);
    const [isLoading, setIsLoading] = useState(true);
    const [error, setError] = useState<string | null>(null);
    const [searchTerm, setSearchTerm] = useState('');
    const [osFilter, setOsFilter] = useState<'all' | 'windows' | 'macos' | 'linux'>('all');

    // Load unpaired devices and active sessions
    const loadPairingData = async () => {
        try {
            setIsLoading(true);
            setError(null);

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

            setUnpairedDevices(unpaired);

            // Get active pairing sessions
            const sessions = await invoke<PairingSession[]>('get_active_pairing_sessions');
            setActiveSessions(sessions);

        } catch (err) {
            console.error('Failed to load pairing data:', err);
            setError('Failed to load devices');
        } finally {
            setIsLoading(false);
        }
    };

    // Filter devices based on search and OS filter
    const getFilteredDevices = () => {
        let filtered = unpairedDevices;

        // Apply search filter
        if (searchTerm) {
            filtered = filtered.filter(device =>
                device.name.toLowerCase().includes(searchTerm.toLowerCase()) ||
                device.address.includes(searchTerm) ||
                (device.platform && device.platform.toLowerCase().includes(searchTerm.toLowerCase()))
            );
        }

        // Apply OS filter
        if (osFilter !== 'all') {
            filtered = filtered.filter(device => {
                const platform = device.platform?.toLowerCase();
                switch (osFilter) {
                    case 'windows':
                        return platform === 'windows';
                    case 'macos':
                        return platform === 'macos' || platform === 'darwin';
                    case 'linux':
                        return platform === 'linux';
                    default:
                        return true;
                }
            });
        }

        return filtered;
    };

    // Initialize data and set up refresh interval
    useEffect(() => {
        loadPairingData();

        // Refresh every 2 seconds for real-time updates
        const interval = setInterval(loadPairingData, 2000);
        return () => clearInterval(interval);
    }, []);

    // Initiate pairing with a device
    const handlePairDevice = async (deviceId: string) => {
        try {
            await invoke('initiate_pairing', { deviceId });
            loadPairingData(); // Refresh to show new session
        } catch (err) {
            console.error('Failed to initiate pairing:', err);
            setError(`Failed to pair with device: ${err}`);
        }
    };

    // Confirm pairing (user verified PIN)
    const handleConfirmPairing = async (sessionId: string) => {
        try {
            await invoke('confirm_pairing', { sessionId });
            loadPairingData(); // Refresh session state
        } catch (err) {
            console.error('Failed to confirm pairing:', err);
            setError(`Failed to confirm pairing: ${err}`);
        }
    };

    // Reject pairing
    const handleRejectPairing = async (sessionId: string) => {
        try {
            await invoke('reject_pairing', { 
                sessionId, 
                reason: 'User rejected' 
            });
            loadPairingData(); // Refresh session state
        } catch (err) {
            console.error('Failed to reject pairing:', err);
            setError(`Failed to reject pairing: ${err}`);
        }
    };

    // Get device type icon
    const getDeviceIcon = (platform?: string) => {
        switch (platform?.toLowerCase()) {
            case 'android':
            case 'ios':
                return <Smartphone className="w-5 h-5" />;
            case 'windows':
            case 'macos':
            case 'linux':
                return <Monitor className="w-5 h-5" />;
            default:
                return <Tablet className="w-5 h-5" />;
        }
    };

    // Get session status info
    const getSessionStatus = (session: PairingSession) => {
        if (typeof session.state === 'object' && 'Failed' in session.state) {
            return {
                icon: <XCircle className="w-5 h-5 text-red-500" />,
                text: `Failed: ${session.state.Failed}`,
                color: 'text-red-500'
            };
        }

        switch (session.state) {
            case 'Initiated':
                return {
                    icon: <Clock className="w-5 h-5 text-blue-500" />,
                    text: 'Waiting for response...',
                    color: 'text-blue-500'
                };
            case 'Challenging':
                return {
                    icon: <Shield className="w-5 h-5 text-yellow-500" />,
                    text: `Verify PIN: ${session.pin}`,
                    color: 'text-yellow-500'
                };
            case 'AwaitingConfirm':
                return {
                    icon: <Clock className="w-5 h-5 text-blue-500" />,
                    text: 'Waiting for other device...',
                    color: 'text-blue-500'
                };
            case 'Confirmed':
                return {
                    icon: <RefreshCw className="w-5 h-5 text-blue-500 animate-spin" />,
                    text: 'Completing pairing...',
                    color: 'text-blue-500'
                };
            case 'Completed':
                return {
                    icon: <CheckCircle className="w-5 h-5 text-green-500" />,
                    text: 'Successfully paired!',
                    color: 'text-green-500'
                };
            case 'Timeout':
                return {
                    icon: <XCircle className="w-5 h-5 text-red-500" />,
                    text: 'Session timed out',
                    color: 'text-red-500'
                };
            default:
                return {
                    icon: <Clock className="w-5 h-5 text-gray-500" />,
                    text: 'Unknown status',
                    color: 'text-gray-500'
                };
        }
    };

    // Format time ago
    const formatTimeAgo = (timestamp: number) => {
        const now = Math.floor(Date.now() / 1000);
        const seconds = now - timestamp;
        
        if (seconds < 60) return 'Just now';
        if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
        if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
        return `${Math.floor(seconds / 86400)}d ago`;
    };

    const filteredDevices = getFilteredDevices();

    return (
        <div className="space-y-6">
            {/* Header */}
            <div className="space-y-4">
                <div className="flex items-center justify-between">
                    <div>
                        <h2 className="text-xl font-semibold text-white">Device Pairing</h2>
                        <p className="text-sm text-gray-400">
                            Pair with devices to enable secure file sharing
                        </p>
                    </div>
                    <button
                        onClick={() => { onRefresh(); loadPairingData(); }}
                        className="flex items-center gap-2 px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors"
                    >
                        <RefreshCw className="w-4 h-4" />
                        Refresh
                    </button>
                </div>

                {/* Search and Filters */}
                <div className="flex flex-col sm:flex-row gap-4">
                    {/* Search Bar */}
                    <div className="relative flex-1">
                        <Search className="absolute left-3 top-1/2 transform -translate-y-1/2 text-gray-400 w-4 h-4" />
                        <input
                            type="text"
                            placeholder="Search devices by name, address..."
                            value={searchTerm}
                            onChange={(e) => setSearchTerm(e.target.value)}
                            className="w-full pl-10 pr-4 py-2 bg-white/10 border border-white/20 rounded-lg text-white placeholder-gray-400 focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent"
                        />
                    </div>

                    {/* OS Filter */}
                    <div className="relative">
                        <select
                            value={osFilter}
                            onChange={(e) => setOsFilter(e.target.value as any)}
                            className="appearance-none bg-white/10 border border-white/20 rounded-lg px-4 py-2 pr-8 text-white focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent"
                        >
                            <option value="all" className="bg-gray-800">All OS</option>
                            <option value="windows" className="bg-gray-800">Windows</option>
                            <option value="macos" className="bg-gray-800">macOS</option>
                            <option value="linux" className="bg-gray-800">Linux</option>
                        </select>
                        <Filter className="absolute right-2 top-1/2 transform -translate-y-1/2 text-gray-400 w-4 h-4 pointer-events-none" />
                    </div>
                </div>
            </div>

            {/* Error Message */}
            {error && (
                <motion.div
                    initial={{ opacity: 0, y: -10 }}
                    animate={{ opacity: 1, y: 0 }}
                    className="bg-red-500/10 border border-red-500/20 rounded-lg p-4 flex items-center gap-3"
                >
                    <AlertCircle className="w-5 h-5 text-red-400" />
                    <span className="text-red-300">{error}</span>
                    <button
                        onClick={() => setError(null)}
                        className="ml-auto text-red-400 hover:text-red-300"
                    >
                        <XCircle className="w-5 h-5" />
                    </button>
                </motion.div>
            )}

            {/* Active Pairing Sessions */}
            <AnimatePresence>
                {activeSessions.length > 0 && (
                    <motion.div
                        initial={{ opacity: 0, height: 0 }}
                        animate={{ opacity: 1, height: 'auto' }}
                        exit={{ opacity: 0, height: 0 }}
                        className="space-y-3"
                    >
                        <h3 className="text-lg font-medium text-white">Active Pairing Sessions</h3>
                        {activeSessions.map((session) => {
                            const status = getSessionStatus(session);
                            return (
                                <motion.div
                                    key={session.session_id}
                                    layout
                                    className="bg-white/10 border border-white/20 rounded-lg p-4 backdrop-blur-sm"
                                >
                                    <div className="flex items-center justify-between">
                                        <div className="flex items-center gap-3">
                                            <div className="text-gray-300">
                                                {getDeviceIcon()}
                                            </div>
                                            <div>
                                                <h4 className="font-medium text-white">{session.peer_name}</h4>
                                                <div className={`flex items-center gap-2 text-sm ${status.color}`}>
                                                    {status.icon}
                                                    {status.text}
                                                </div>
                                            </div>
                                        </div>
                                        
                                        <div className="flex items-center gap-3">
                                            {session.remaining_seconds > 0 && (
                                                <span className="text-sm text-gray-500">
                                                    {Math.floor(session.remaining_seconds / 60)}:
                                                    {(session.remaining_seconds % 60).toString().padStart(2, '0')}
                                                </span>
                                            )}
                                            
                                            {session.state === 'Challenging' && (
                                                <div className="flex gap-2">
                                                    <button
                                                        onClick={() => handleConfirmPairing(session.session_id)}
                                                        className="px-3 py-1 bg-green-500 text-white rounded text-sm hover:bg-green-600 transition-colors"
                                                    >
                                                        Confirm
                                                    </button>
                                                    <button
                                                        onClick={() => handleRejectPairing(session.session_id)}
                                                        className="px-3 py-1 bg-red-500 text-white rounded text-sm hover:bg-red-600 transition-colors"
                                                    >
                                                        Reject
                                                    </button>
                                                </div>
                                            )}
                                        </div>
                                    </div>
                                </motion.div>
                            );
                        })}
                    </motion.div>
                )}
            </AnimatePresence>

            {/* Discovered Devices */}
            <div className="space-y-3">
                <h3 className="text-lg font-medium text-white">Discovered Devices</h3>

                {isLoading ? (
                    <div className="space-y-3">
                        {[...Array(3)].map((_, i) => (
                            <div key={i} className="bg-white/10 rounded-lg h-16 animate-pulse" />
                        ))}
                    </div>
                ) : filteredDevices.length === 0 ? (
                    <motion.div
                        initial={{ opacity: 0 }}
                        animate={{ opacity: 1 }}
                        className="text-center py-8 text-gray-400"
                    >
                        <Wifi className="w-12 h-12 mx-auto mb-3 text-gray-500" />
                        <p className="text-white">No unpaired devices found</p>
                        <p className="text-sm">Make sure other devices are running Fileshare</p>
                    </motion.div>
                ) : (
                    <motion.div
                        className="space-y-3"
                        initial={{ opacity: 0 }}
                        animate={{ opacity: 1 }}
                    >
                        {filteredDevices.map((device, index) => (
                            <motion.div
                                key={device.id}
                                initial={{ opacity: 0, y: 20 }}
                                animate={{ opacity: 1, y: 0 }}
                                transition={{ delay: index * 0.1 }}
                                className="bg-white/10 border border-white/20 rounded-lg p-4 hover:bg-white/15 transition-all backdrop-blur-sm"
                            >
                                <div className="flex items-center justify-between">
                                    <div className="flex items-center gap-3">
                                        <div className="text-gray-300">
                                            {getDeviceIcon(device.platform)}
                                        </div>
                                        <div>
                                            <h4 className="font-medium text-white">{device.name}</h4>
                                            <div className="flex items-center gap-4 text-sm text-gray-400">
                                                <span>{device.address}</span>
                                                <span>v{device.version}</span>
                                                {device.platform && (
                                                    <span className="capitalize">{device.platform}</span>
                                                )}
                                                <span>{formatTimeAgo(device.last_seen)}</span>
                                            </div>
                                        </div>
                                    </div>

                                    <button
                                        onClick={() => handlePairDevice(device.id)}
                                        className="px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors flex items-center gap-2"
                                    >
                                        <Shield className="w-4 h-4" />
                                        Pair
                                    </button>
                                </div>
                            </motion.div>
                        ))}
                    </motion.div>
                )}
            </div>
        </div>
    );
};

export default PairingTab;