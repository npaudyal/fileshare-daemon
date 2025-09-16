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
    RefreshCw
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

    // Initialize data and set up refresh interval
    useEffect(() => {
        loadPairingData();
        
        // Refresh every 5 seconds to update session timers
        const interval = setInterval(loadPairingData, 5000);
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

    return (
        <div className="space-y-6">
            {/* Header */}
            <div className="flex items-center justify-between">
                <div>
                    <h2 className="text-xl font-semibold text-gray-900">Device Pairing</h2>
                    <p className="text-sm text-gray-600">
                        Pair with devices to enable secure file sharing
                    </p>
                </div>
                <button
                    onClick={() => { onRefresh(); loadPairingData(); }}
                    className="flex items-center gap-2 px-3 py-2 bg-blue-500 text-white rounded-lg hover:bg-blue-600 transition-colors"
                >
                    <RefreshCw className="w-4 h-4" />
                    Refresh
                </button>
            </div>

            {/* Error Message */}
            {error && (
                <motion.div 
                    initial={{ opacity: 0, y: -10 }}
                    animate={{ opacity: 1, y: 0 }}
                    className="bg-red-50 border border-red-200 rounded-lg p-4 flex items-center gap-3"
                >
                    <AlertCircle className="w-5 h-5 text-red-500" />
                    <span className="text-red-700">{error}</span>
                    <button 
                        onClick={() => setError(null)}
                        className="ml-auto text-red-500 hover:text-red-700"
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
                        <h3 className="text-lg font-medium text-gray-900">Active Pairing Sessions</h3>
                        {activeSessions.map((session) => {
                            const status = getSessionStatus(session);
                            return (
                                <motion.div
                                    key={session.session_id}
                                    layout
                                    className="bg-white border border-gray-200 rounded-lg p-4 shadow-sm"
                                >
                                    <div className="flex items-center justify-between">
                                        <div className="flex items-center gap-3">
                                            {getDeviceIcon()}
                                            <div>
                                                <h4 className="font-medium text-gray-900">{session.peer_name}</h4>
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
                <h3 className="text-lg font-medium text-gray-900">Discovered Devices</h3>
                
                {isLoading ? (
                    <div className="space-y-3">
                        {[...Array(3)].map((_, i) => (
                            <div key={i} className="bg-gray-100 rounded-lg h-16 animate-pulse" />
                        ))}
                    </div>
                ) : unpairedDevices.length === 0 ? (
                    <motion.div
                        initial={{ opacity: 0 }}
                        animate={{ opacity: 1 }}
                        className="text-center py-8 text-gray-500"
                    >
                        <Wifi className="w-12 h-12 mx-auto mb-3 text-gray-300" />
                        <p>No unpaired devices found</p>
                        <p className="text-sm">Make sure other devices are running Fileshare</p>
                    </motion.div>
                ) : (
                    <motion.div 
                        className="space-y-3"
                        initial={{ opacity: 0 }}
                        animate={{ opacity: 1 }}
                    >
                        {unpairedDevices.map((device, index) => (
                            <motion.div
                                key={device.id}
                                initial={{ opacity: 0, y: 20 }}
                                animate={{ opacity: 1, y: 0 }}
                                transition={{ delay: index * 0.1 }}
                                className="bg-white border border-gray-200 rounded-lg p-4 hover:shadow-md transition-shadow"
                            >
                                <div className="flex items-center justify-between">
                                    <div className="flex items-center gap-3">
                                        {getDeviceIcon(device.platform)}
                                        <div>
                                            <h4 className="font-medium text-gray-900">{device.name}</h4>
                                            <div className="flex items-center gap-4 text-sm text-gray-500">
                                                <span>{device.address}</span>
                                                <span>v{device.version}</span>
                                                <span>{formatTimeAgo(device.last_seen)}</span>
                                            </div>
                                        </div>
                                    </div>
                                    
                                    <button
                                        onClick={() => handlePairDevice(device.id)}
                                        className="px-4 py-2 bg-blue-500 text-white rounded-lg hover:bg-blue-600 transition-colors flex items-center gap-2"
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