import React, { useState, useEffect } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import { invoke } from '@tauri-apps/api/core';
import {
    Link2,
    Search,
    Monitor,
    Smartphone,
    Laptop,
    Tablet,
    Server,
    X,
    Info,
    AlertCircle
} from 'lucide-react';
import PinDisplay from './PinDisplay';
import { useToast } from '../hooks/useToast';
import { useDebounce } from '../hooks/useDebounce';
import { FadeIn } from './AnimatedComponents';

interface DeviceInfo {
    id: string;
    name: string;
    device_type: string;
    address: string;
    version: string;
    platform?: string;
    last_seen: number;
}

interface PairingPin {
    code: string;
    generated_at: number;
    expires_at: number;
}

interface FastPairingResult {
    success: boolean;
    remote_device_id?: string;
    remote_device_name?: string;
    error_message?: string;
}

interface PairingTabProps {
    onPairComplete: () => void;
}

const PairingTab: React.FC<PairingTabProps> = ({ onPairComplete }) => {
    const [currentPin, setCurrentPin] = useState<PairingPin | null>(null);
    const [unpairedDevices, setUnpairedDevices] = useState<DeviceInfo[]>([]);
    const [searchTerm, setSearchTerm] = useState('');
    const [platformFilter, setPlatformFilter] = useState<'all' | 'mac' | 'windows' | 'linux'>('all');
    const [showPairingModal, setShowPairingModal] = useState(false);
    const [selectedDevice, setSelectedDevice] = useState<DeviceInfo | null>(null);
    const [pairingPin, setPairingPin] = useState('');
    const [isPairing, setIsPairing] = useState(false);
    const [isLoading, setIsLoading] = useState(true);

    const { addToast } = useToast();
    const debouncedSearchTerm = useDebounce(searchTerm, 300);

    // Load current PIN
    const loadCurrentPin = async () => {
        try {
            const pin = await invoke<PairingPin>('get_current_pin');
            setCurrentPin(pin);
        } catch (error) {
            console.error('Failed to load PIN:', error);
            addToast('error', 'Error', 'Failed to load pairing PIN');
        }
    };

    // Load unpaired devices
    const loadUnpairedDevices = async () => {
        try {
            const devices = await invoke<DeviceInfo[]>('get_unpaired_devices');
            setUnpairedDevices(devices);
        } catch (error) {
            console.error('Failed to load unpaired devices:', error);
            addToast('error', 'Error', 'Failed to load devices');
        } finally {
            setIsLoading(false);
        }
    };

    // Refresh PIN
    const handleRefreshPin = async () => {
        try {
            const pin = await invoke<PairingPin>('refresh_pin');
            setCurrentPin(pin);
            addToast('success', 'PIN Refreshed', 'New PIN generated successfully');
        } catch (error) {
            console.error('Failed to refresh PIN:', error);
            addToast('error', 'Error', 'Failed to refresh PIN');
        }
    };

    // Initiate fast pairing (new Bluetooth-style system)
    const handlePairDevice = async () => {
        if (!selectedDevice || !pairingPin) return;

        setIsPairing(true);
        try {
            // Use the new fast_pairing command instead of initiate_pairing
            const result = await invoke<FastPairingResult>('fast_pairing', {
                deviceId: selectedDevice.id,  // Note: using camelCase as expected by Tauri
                pin: pairingPin
            });
            
            console.log('Fast pairing result:', result);
            
            // Check if pairing was successful
            if (result.success) {
                addToast('success', 'Fast Pairing Successful! 🚀', `Quickly paired with ${selectedDevice.name}`);
                setShowPairingModal(false);
                setPairingPin('');
                setSelectedDevice(null);
                
                // Reload devices and notify parent
                await loadUnpairedDevices();
                onPairComplete();
            } else {
                const errorMsg = result.error_message || 'Unknown error';
                addToast('error', 'Fast Pairing Failed', errorMsg);
            }
        } catch (error) {
            console.error('Failed to pair device:', error);
            addToast('error', 'Fast Pairing Failed', `${error}`);
        } finally {
            setIsPairing(false);
        }
    };

    // Filter devices
    const getFilteredDevices = () => {
        let filtered = unpairedDevices;

        // Search filter
        if (debouncedSearchTerm) {
            filtered = filtered.filter(device =>
                device.name.toLowerCase().includes(debouncedSearchTerm.toLowerCase()) ||
                device.address.includes(debouncedSearchTerm)
            );
        }

        // Platform filter
        if (platformFilter !== 'all') {
            filtered = filtered.filter(device => {
                const platform = device.platform?.toLowerCase() || '';
                switch (platformFilter) {
                    case 'mac':
                        return platform.includes('mac') || platform.includes('darwin');
                    case 'windows':
                        return platform.includes('windows') || platform.includes('win');
                    case 'linux':
                        return platform.includes('linux') || platform.includes('ubuntu');
                    default:
                        return true;
                }
            });
        }

        return filtered;
    };

    // Get device icon
    const getDeviceIcon = (deviceType: string) => {
        const iconClass = "w-6 h-6 text-gray-400";
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
            case 'desktop':
            default:
                return <Monitor className={iconClass} />;
        }
    };

    // Format time ago
    const getTimeAgo = (timestamp: number) => {
        const now = Math.floor(Date.now() / 1000);
        const diff = now - timestamp;

        if (diff < 60) return 'Just now';
        if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
        if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
        return `${Math.floor(diff / 86400)}d ago`;
    };

    useEffect(() => {
        loadCurrentPin();
        loadUnpairedDevices();

        // Refresh devices every 10 seconds
        const interval = setInterval(loadUnpairedDevices, 10000);
        return () => clearInterval(interval);
    }, []);

    const filteredDevices = getFilteredDevices();

    return (
        <FadeIn className="h-full flex flex-col">
            {/* PIN Display Section */}
            {currentPin && (
                <div className="mb-6">
                    <PinDisplay
                        pin={currentPin.code}
                        expiresAt={currentPin.expires_at}
                        onRefresh={handleRefreshPin}
                    />
                </div>
            )}

            {/* Available Devices Section */}
            <div className="flex-1 bg-white/5 rounded-lg p-4 backdrop-blur-sm">
                {/* Header */}
                <div className="mb-4">
                    <h3 className="text-lg font-semibold text-white mb-3">Available Devices</h3>
                    
                    {/* Search and Filters */}
                    <div className="flex space-x-3">
                        {/* Search Bar */}
                        <div className="flex-1 relative">
                            <Search className="absolute left-3 top-1/2 transform -translate-y-1/2 w-4 h-4 text-gray-400" />
                            <input
                                type="text"
                                value={searchTerm}
                                onChange={(e) => setSearchTerm(e.target.value)}
                                placeholder="Search devices..."
                                className="w-full pl-10 pr-4 py-2 bg-white/10 rounded-lg text-white placeholder-gray-400 focus:outline-none focus:ring-2 focus:ring-blue-500"
                            />
                        </div>

                        {/* Platform Filter */}
                        <div className="flex space-x-1 bg-white/10 rounded-lg p-1">
                            {(['all', 'mac', 'windows', 'linux'] as const).map((filter) => (
                                <button
                                    key={filter}
                                    onClick={() => setPlatformFilter(filter)}
                                    className={`px-3 py-1.5 rounded-md text-xs font-medium transition-all ${
                                        platformFilter === filter
                                            ? 'bg-blue-500 text-white'
                                            : 'text-gray-400 hover:text-white'
                                    }`}
                                >
                                    {filter.charAt(0).toUpperCase() + filter.slice(1)}
                                </button>
                            ))}
                        </div>
                    </div>
                </div>

                {/* Device List */}
                <div className="space-y-2 max-h-96 overflow-y-auto">
                    {isLoading ? (
                        <div className="text-center py-8 text-gray-400">
                            Loading devices...
                        </div>
                    ) : filteredDevices.length === 0 ? (
                        <div className="text-center py-8">
                            <AlertCircle className="w-12 h-12 text-gray-500 mx-auto mb-2" />
                            <p className="text-gray-400">No unpaired devices found</p>
                            <p className="text-sm text-gray-500 mt-1">
                                Make sure other devices are running and on the same network
                            </p>
                        </div>
                    ) : (
                        <AnimatePresence>
                            {filteredDevices.map((device) => (
                                <motion.div
                                    key={device.id}
                                    initial={{ opacity: 0, y: 20 }}
                                    animate={{ opacity: 1, y: 0 }}
                                    exit={{ opacity: 0, y: -20 }}
                                    whileHover={{ scale: 1.02 }}
                                    className="bg-white/10 backdrop-blur-sm rounded-lg p-4 border border-white/20 hover:border-blue-400/50 transition-all"
                                >
                                    <div className="flex items-center justify-between">
                                        <div className="flex items-center space-x-3">
                                            {getDeviceIcon(device.device_type)}
                                            <div>
                                                <h4 className="text-white font-medium">{device.name}</h4>
                                                <div className="flex items-center space-x-3 text-xs text-gray-400">
                                                    <span>{device.address}</span>
                                                    <span>•</span>
                                                    <span>{getTimeAgo(device.last_seen)}</span>
                                                    {device.platform && (
                                                        <>
                                                            <span>•</span>
                                                            <span>{device.platform}</span>
                                                        </>
                                                    )}
                                                </div>
                                            </div>
                                        </div>
                                        
                                        <div className="flex items-center space-x-2">
                                            <motion.button
                                                whileHover={{ scale: 1.1 }}
                                                whileTap={{ scale: 0.9 }}
                                                className="p-2 rounded-lg hover:bg-white/10 transition-colors group"
                                                title="Device Info"
                                            >
                                                <Info className="w-4 h-4 text-gray-400 group-hover:text-white" />
                                            </motion.button>
                                            
                                            <motion.button
                                                whileHover={{ scale: 1.05 }}
                                                whileTap={{ scale: 0.95 }}
                                                onClick={() => {
                                                    setSelectedDevice(device);
                                                    setShowPairingModal(true);
                                                }}
                                                className="px-4 py-2 bg-blue-500 text-white rounded-lg hover:bg-blue-600 transition-colors flex items-center space-x-2"
                                            >
                                                <Link2 className="w-4 h-4" />
                                                <span>Pair</span>
                                            </motion.button>
                                        </div>
                                    </div>
                                </motion.div>
                            ))}
                        </AnimatePresence>
                    )}
                </div>
            </div>

            {/* Pairing Modal */}
            <AnimatePresence>
                {showPairingModal && selectedDevice && (
                    <motion.div
                        initial={{ opacity: 0 }}
                        animate={{ opacity: 1 }}
                        exit={{ opacity: 0 }}
                        className="fixed inset-0 bg-black/50 backdrop-blur-sm flex items-center justify-center z-50"
                        onClick={() => !isPairing && setShowPairingModal(false)}
                    >
                        <motion.div
                            initial={{ scale: 0.95, opacity: 0 }}
                            animate={{ scale: 1, opacity: 1 }}
                            exit={{ scale: 0.95, opacity: 0 }}
                            className="bg-gray-800/90 backdrop-blur-md rounded-2xl p-6 max-w-md w-full mx-4 border border-white/20"
                            onClick={(e) => e.stopPropagation()}
                        >
                            <div className="flex items-center justify-between mb-4">
                                <div className="flex items-center space-x-2">
                                    <h3 className="text-xl font-semibold text-white">Fast Pair Device</h3>
                                    <span className="px-2 py-1 bg-blue-500/20 text-blue-300 text-xs rounded-full">
                                        ⚡ Bluetooth-style
                                    </span>
                                </div>
                                <button
                                    onClick={() => !isPairing && setShowPairingModal(false)}
                                    className="p-1 rounded-lg hover:bg-white/10 transition-colors"
                                    disabled={isPairing}
                                >
                                    <X className="w-5 h-5 text-gray-400" />
                                </button>
                            </div>

                            <div className="mb-6">
                                <p className="text-gray-300 mb-2">
                                    Enter the PIN displayed on <strong>{selectedDevice.name}</strong>
                                </p>
                                <p className="text-sm text-blue-300/60 mb-1">
                                    ⚡ Fast pairing will complete in ~5 seconds
                                </p>
                                <p className="text-sm text-gray-500">
                                    Device: {selectedDevice.address}
                                </p>
                            </div>

                            <div className="mb-6">
                                <label className="block text-sm font-medium text-gray-300 mb-2">
                                    Pairing PIN
                                </label>
                                <input
                                    type="text"
                                    value={pairingPin}
                                    onChange={(e) => setPairingPin(e.target.value.replace(/\D/g, '').slice(0, 6))}
                                    placeholder="Enter 6-digit PIN"
                                    className="w-full px-4 py-3 bg-white/10 rounded-lg text-white text-center text-2xl font-mono tracking-wider placeholder-gray-500 focus:outline-none focus:ring-2 focus:ring-blue-500"
                                    maxLength={6}
                                    disabled={isPairing}
                                />
                            </div>

                            <div className="flex space-x-3">
                                <button
                                    onClick={() => !isPairing && setShowPairingModal(false)}
                                    className="flex-1 px-4 py-2 bg-white/10 text-white rounded-lg hover:bg-white/20 transition-colors"
                                    disabled={isPairing}
                                >
                                    Cancel
                                </button>
                                <button
                                    onClick={handlePairDevice}
                                    disabled={isPairing || pairingPin.length !== 6}
                                    className={`flex-1 px-4 py-2 rounded-lg font-medium transition-colors flex items-center justify-center space-x-2 ${
                                        isPairing || pairingPin.length !== 6
                                            ? 'bg-gray-700 text-gray-400 cursor-not-allowed'
                                            : 'bg-blue-500 text-white hover:bg-blue-600'
                                    }`}
                                >
                                    {isPairing ? (
                                        <>
                                            <div className="animate-spin rounded-full h-4 w-4 border-2 border-white border-t-transparent" />
                                            <span>Fast Pairing...</span>
                                        </>
                                    ) : (
                                        <>
                                            <span className="text-base">⚡</span>
                                            <span>Fast Pair</span>
                                        </>
                                    )}
                                </button>
                            </div>
                        </motion.div>
                    </motion.div>
                )}
            </AnimatePresence>
        </FadeIn>
    );
};

export default PairingTab;