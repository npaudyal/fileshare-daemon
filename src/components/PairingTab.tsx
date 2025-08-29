import React, { useState, useEffect } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import { invoke } from '@tauri-apps/api/core';
import {
    Link2,
    Smartphone,
    Laptop,
    Monitor,
    Tablet,
    Server,
    RefreshCw,
    Wifi,
    Clock,
    Globe,
    AlertCircle,
    CheckCircle,
    Shield,
    Key,
    X
} from 'lucide-react';

interface DeviceInfo {
    id: string;
    name: string;
    device_type: string;
    is_paired: boolean;
    is_connected: boolean;
    is_blocked: boolean;
    trust_level: string;
    address: string;
    version: string;
    platform?: string;
    last_seen: number;
}

interface PairingTabProps {
    devices: DeviceInfo[];
    onRefresh: () => void;
    onStartPairing: (deviceId: string) => void;
    isRefreshing: boolean;
}

interface PairingSession {
    deviceId: string;
    sessionId?: string;
    pin?: string;
    status: 'requesting' | 'showing_pin' | 'waiting_for_pin' | 'success' | 'error';
    error?: string;
    expiresAt?: number;
}

const PairingTab: React.FC<PairingTabProps> = ({ 
    devices, 
    onRefresh, 
    onStartPairing, 
    isRefreshing 
}) => {
    const [pairingSession, setPairingSession] = useState<PairingSession | null>(null);
    const [enteredPin, setEnteredPin] = useState('');

    // Clean up expired sessions periodically
    useEffect(() => {
        const interval = setInterval(async () => {
            try {
                await invoke('cleanup_expired_sessions');
            } catch (error) {
                console.error('Failed to cleanup expired sessions:', error);
            }
        }, 30000); // Clean up every 30 seconds

        return () => clearInterval(interval);
    }, []);

    // Auto-cancel expired pairing sessions
    useEffect(() => {
        if (pairingSession?.expiresAt) {
            const now = Math.floor(Date.now() / 1000);
            const timeLeft = pairingSession.expiresAt - now;
            
            if (timeLeft <= 0) {
                handleCancelPairing();
            } else {
                const timeout = setTimeout(() => {
                    setPairingSession(prev => prev ? {
                        ...prev,
                        status: 'error',
                        error: 'Session expired'
                    } : null);
                }, timeLeft * 1000);

                return () => clearTimeout(timeout);
            }
        }
    }, [pairingSession?.expiresAt]);

    // Get unpaired devices that aren't blocked
    const unpairedDevices = devices.filter(device => 
        !device.is_paired && !device.is_blocked
    );

    // Device icon helper
    const getDeviceIcon = (deviceType: string, _platform: string | undefined) => {
        const iconClass = "w-5 h-5 text-gray-400";
        
        switch (deviceType.toLowerCase()) {
            case 'phone':
            case 'mobile':
                return <Smartphone className={iconClass} />;
            case 'tablet':
                return <Tablet className={iconClass} />;
            case 'laptop':
                return <Laptop className={iconClass} />;
            case 'server':
                return <Server className={iconClass} />;
            default:
                return <Monitor className={iconClass} />;
        }
    };

    // Format time since last seen
    const formatLastSeen = (timestamp: number) => {
        const now = Date.now() / 1000;
        const diff = now - timestamp;
        
        if (diff < 60) return 'Just now';
        if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
        if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
        return `${Math.floor(diff / 86400)}d ago`;
    };

    // Handle pairing initiation
    const handlePairDevice = async (deviceId: string) => {
        setPairingSession({
            deviceId,
            status: 'requesting'
        });
        
        try {
            // Call the backend to start pairing
            const response = await invoke<{
                session_id: string;
                pin: string;
                expires_at: number;
                status: string;
            }>('request_pairing', { deviceId });
            
            setPairingSession({
                deviceId,
                sessionId: response.session_id,
                pin: response.pin,
                status: 'showing_pin',
                expiresAt: response.expires_at
            });
            
            // Also call the parent's handler for any additional processing
            onStartPairing(deviceId);
            
        } catch (error) {
            setPairingSession({
                deviceId,
                status: 'error',
                error: error instanceof Error ? error.message : 'Failed to start pairing'
            });
        }
    };

    // Handle PIN submission
    const handleSubmitPin = async () => {
        if (!pairingSession || !pairingSession.sessionId || enteredPin.length !== 6) return;
        
        try {
            const response = await invoke<{
                success: boolean;
                device_id?: string;
                message?: string;
                error?: string;
            }>('verify_pairing_pin', { 
                sessionId: pairingSession.sessionId, 
                pin: enteredPin 
            });
            
            if (response.success) {
                setPairingSession(prev => prev ? { ...prev, status: 'success' } : null);
                setEnteredPin('');
                setTimeout(() => {
                    setPairingSession(null);
                    onRefresh(); // Refresh to show the newly paired device
                }, 2000);
            } else {
                setPairingSession(prev => prev ? {
                    ...prev,
                    status: 'error',
                    error: response.error || 'Invalid PIN'
                } : null);
            }
        } catch (error) {
            setPairingSession(prev => prev ? {
                ...prev,
                status: 'error',
                error: error instanceof Error ? error.message : 'Failed to verify PIN'
            } : null);
        }
    };

    // Cancel pairing
    const handleCancelPairing = async () => {
        if (pairingSession?.sessionId) {
            try {
                await invoke('cancel_pairing', { sessionId: pairingSession.sessionId });
            } catch (error) {
                console.error('Failed to cancel pairing session:', error);
            }
        }
        
        setPairingSession(null);
        setEnteredPin('');
    };

    return (
        <div className="space-y-6">
            {/* Header */}
            <div className="flex items-center justify-between">
                <div>
                    <h2 className="text-xl font-semibold text-white flex items-center gap-2">
                        <Link2 className="w-5 h-5 text-blue-400" />
                        Device Pairing
                    </h2>
                    <p className="text-gray-400 mt-1">
                        Discover and pair new devices on your network
                    </p>
                </div>
                
                <motion.button
                    whileHover={{ scale: 1.05 }}
                    whileTap={{ scale: 0.95 }}
                    onClick={onRefresh}
                    disabled={isRefreshing}
                    className="px-4 py-2 bg-blue-600 hover:bg-blue-700 text-white rounded-lg flex items-center gap-2 disabled:opacity-50"
                >
                    <RefreshCw className={`w-4 h-4 ${isRefreshing ? 'animate-spin' : ''}`} />
                    {isRefreshing ? 'Scanning...' : 'Scan'}
                </motion.button>
            </div>

            {/* Unpaired devices list */}
            <div className="space-y-3">
                <AnimatePresence>
                    {unpairedDevices.length === 0 ? (
                        <motion.div
                            initial={{ opacity: 0, y: 20 }}
                            animate={{ opacity: 1, y: 0 }}
                            className="text-center py-12 text-gray-400"
                        >
                            <Wifi className="w-12 h-12 mx-auto mb-4 opacity-50" />
                            <p className="text-lg font-medium mb-2">No devices found</p>
                            <p className="text-sm">
                                {isRefreshing 
                                    ? 'Scanning for devices...' 
                                    : 'Click "Scan" to search for nearby devices'
                                }
                            </p>
                        </motion.div>
                    ) : (
                        unpairedDevices.map((device) => (
                            <motion.div
                                key={device.id}
                                initial={{ opacity: 0, y: 20 }}
                                animate={{ opacity: 1, y: 0 }}
                                exit={{ opacity: 0, y: -20 }}
                                className="bg-gray-800/50 border border-gray-700 rounded-lg p-4 hover:border-gray-600 transition-colors"
                            >
                                <div className="flex items-center justify-between">
                                    <div className="flex items-center gap-3">
                                        {getDeviceIcon(device.device_type, device.platform)}
                                        <div>
                                            <h3 className="text-white font-medium">{device.name}</h3>
                                            <div className="flex items-center gap-4 text-sm text-gray-400 mt-1">
                                                <span className="flex items-center gap-1">
                                                    <Globe className="w-3 h-3" />
                                                    {device.address}
                                                </span>
                                                <span className="flex items-center gap-1">
                                                    <Clock className="w-3 h-3" />
                                                    {formatLastSeen(device.last_seen)}
                                                </span>
                                                {device.platform && (
                                                    <span className="text-gray-500">
                                                        {device.platform}
                                                    </span>
                                                )}
                                            </div>
                                        </div>
                                    </div>
                                    
                                    <motion.button
                                        whileHover={{ scale: 1.05 }}
                                        whileTap={{ scale: 0.95 }}
                                        onClick={() => handlePairDevice(device.id)}
                                        disabled={pairingSession?.deviceId === device.id}
                                        className="px-4 py-2 bg-green-600 hover:bg-green-700 text-white rounded-lg flex items-center gap-2 disabled:opacity-50"
                                    >
                                        <Shield className="w-4 h-4" />
                                        {pairingSession?.deviceId === device.id ? 'Pairing...' : 'Pair'}
                                    </motion.button>
                                </div>
                            </motion.div>
                        ))
                    )}
                </AnimatePresence>
            </div>

            {/* Pairing Dialog */}
            <AnimatePresence>
                {pairingSession && (
                    <motion.div
                        initial={{ opacity: 0 }}
                        animate={{ opacity: 1 }}
                        exit={{ opacity: 0 }}
                        className="fixed inset-0 bg-black/50 flex items-center justify-center z-50"
                        onClick={handleCancelPairing}
                    >
                        <motion.div
                            initial={{ scale: 0.9, opacity: 0 }}
                            animate={{ scale: 1, opacity: 1 }}
                            exit={{ scale: 0.9, opacity: 0 }}
                            className="bg-gray-800 border border-gray-700 rounded-xl p-6 max-w-md w-full mx-4"
                            onClick={(e) => e.stopPropagation()}
                        >
                            <div className="flex items-center justify-between mb-4">
                                <h3 className="text-lg font-semibold text-white">Device Pairing</h3>
                                <button
                                    onClick={handleCancelPairing}
                                    className="text-gray-400 hover:text-white"
                                >
                                    <X className="w-5 h-5" />
                                </button>
                            </div>

                            {pairingSession.status === 'requesting' && (
                                <div className="text-center py-4">
                                    <motion.div
                                        animate={{ rotate: 360 }}
                                        transition={{ duration: 1, repeat: Infinity, ease: "linear" }}
                                        className="w-8 h-8 border-2 border-blue-400 border-t-transparent rounded-full mx-auto mb-4"
                                    />
                                    <p className="text-white">Initiating pairing...</p>
                                </div>
                            )}

                            {pairingSession.status === 'showing_pin' && pairingSession.pin && (
                                <div className="text-center py-4">
                                    <Key className="w-12 h-12 text-blue-400 mx-auto mb-4" />
                                    <p className="text-white mb-4">Show this PIN on the other device:</p>
                                    <div className="text-3xl font-mono bg-gray-700 rounded-lg p-4 text-center text-blue-400">
                                        {pairingSession.pin}
                                    </div>
                                    <p className="text-gray-400 text-sm mt-4">
                                        Waiting for the other device to enter this PIN...
                                    </p>
                                </div>
                            )}

                            {pairingSession.status === 'waiting_for_pin' && (
                                <div className="text-center py-4">
                                    <Key className="w-12 h-12 text-green-400 mx-auto mb-4" />
                                    <p className="text-white mb-4">Enter the PIN shown on the other device:</p>
                                    <input
                                        type="text"
                                        maxLength={6}
                                        value={enteredPin}
                                        onChange={(e) => setEnteredPin(e.target.value.replace(/\D/g, ''))}
                                        className="w-full text-2xl font-mono text-center bg-gray-700 border border-gray-600 rounded-lg p-3 text-white mb-4"
                                        placeholder="000000"
                                        autoFocus
                                    />
                                    <motion.button
                                        whileHover={{ scale: 1.05 }}
                                        whileTap={{ scale: 0.95 }}
                                        onClick={handleSubmitPin}
                                        disabled={enteredPin.length !== 6}
                                        className="w-full py-2 bg-green-600 hover:bg-green-700 text-white rounded-lg disabled:opacity-50"
                                    >
                                        Verify PIN
                                    </motion.button>
                                </div>
                            )}

                            {pairingSession.status === 'success' && (
                                <div className="text-center py-4">
                                    <CheckCircle className="w-12 h-12 text-green-400 mx-auto mb-4" />
                                    <p className="text-white text-lg">Pairing Successful!</p>
                                    <p className="text-gray-400 mt-2">Device has been added to your paired devices.</p>
                                </div>
                            )}

                            {pairingSession.status === 'error' && (
                                <div className="text-center py-4">
                                    <AlertCircle className="w-12 h-12 text-red-400 mx-auto mb-4" />
                                    <p className="text-white text-lg">Pairing Failed</p>
                                    <p className="text-red-400 mt-2">{pairingSession.error || 'Unknown error occurred'}</p>
                                    <motion.button
                                        whileHover={{ scale: 1.05 }}
                                        whileTap={{ scale: 0.95 }}
                                        onClick={handleCancelPairing}
                                        className="mt-4 px-4 py-2 bg-gray-600 hover:bg-gray-700 text-white rounded-lg"
                                    >
                                        Close
                                    </motion.button>
                                </div>
                            )}
                        </motion.div>
                    </motion.div>
                )}
            </AnimatePresence>
        </div>
    );
};

export default PairingTab;