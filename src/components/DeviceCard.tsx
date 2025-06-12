import React, { useState } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import {
    Smartphone,
    Laptop,
    Monitor,
    Tablet,
    Server,
    Edit2,
    Check,
    X,
    CheckCircle,
    AlertCircle,
    Clock,
    Globe,
    MoreVertical,
    Shield,
    ShieldCheck,
    ShieldX,
    Trash2,
    UserX,
    Eye,
    Ban,
    CheckSquare,
    Square,
    Star,
    Wifi,
    AlertTriangle
} from 'lucide-react';
import DeviceManagement from './DeviceManagement';

interface DeviceInfo {
    id: string;
    name: string;
    display_name?: string;
    device_type: string;
    is_paired: boolean;
    is_connected: boolean;
    is_blocked: boolean;
    trust_level: string;
    last_seen: number;
    first_seen: number;
    connection_count: number;
    address: string;
    version: string;
    platform?: string;
    last_transfer_time?: number;
    total_transfers: number;
}

interface DeviceCardProps {
    device: DeviceInfo;
    isSelected: boolean;
    isFavorite: boolean;
    onSelect: () => void;
    onPair: () => void;
    onUnpair: () => void;
    onBlock: () => void;
    onUnblock: () => void;
    onForget: () => void;
    onRename: (newName: string) => void;
    onToggleFavorite: () => void;
    onConnect: () => void;
    onDisconnect: () => void;
}

const DeviceCard: React.FC<DeviceCardProps> = ({
    device,
    isSelected,
    isFavorite,
    onSelect,
    onPair,
    onUnpair,
    onBlock,
    onUnblock,
    onForget,
    onRename,
    onToggleFavorite,
    onConnect,
    onDisconnect
}) => {
    const [editingDevice, setEditingDevice] = useState(false);
    const [newDeviceName, setNewDeviceName] = useState('');
    const [deviceMenuOpen, setDeviceMenuOpen] = useState(false);
    const [showDeviceDetails, setShowDeviceDetails] = useState(false);

    // Device icon helper
    const getDeviceIcon = (deviceType: string, _platform: string | undefined, isConnected: boolean) => {
        const iconClass = `w-6 h-6 ${isConnected ? 'text-green-500' : 'text-gray-400'}`;

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

    // Trust level indicator
    const getTrustLevelIndicator = (trustLevel: string) => {
        switch (trustLevel) {
            case 'Verified':
                return <ShieldCheck className="w-4 h-4 text-green-500" />;
            case 'Trusted':
                return <Shield className="w-4 h-4 text-blue-500" />;
            case 'Blocked':
                return <ShieldX className="w-4 h-4 text-red-500" />;
            default:
                return <AlertTriangle className="w-4 h-4 text-yellow-500" />;
        }
    };

    // Time formatting helper
    const getTimeAgo = (timestamp: number) => {
        const now = Math.floor(Date.now() / 1000);
        const diff = now - timestamp;

        if (diff < 60) return 'Just now';
        if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
        if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
        return `${Math.floor(diff / 86400)}d ago`;
    };

    const handleRename = () => {
        if (!newDeviceName.trim()) return;

        // Validate name
        if (newDeviceName.length > 50) {
            alert('Device name must be 50 characters or less');
            return;
        }

        if (newDeviceName.match(/[<>:"|?*\/\\]/)) {
            alert('Device name contains forbidden characters');
            return;
        }

        onRename(newDeviceName.trim());
        setEditingDevice(false);
        setNewDeviceName('');
    };

    const startEditing = () => {
        setEditingDevice(true);
        setNewDeviceName(device.display_name || device.name);
    };

    const cancelEditing = () => {
        setEditingDevice(false);
        setNewDeviceName('');
    };

    return (
        <motion.div
            layout
            whileHover={{ y: -2, boxShadow: "0 10px 25px -5px rgba(0, 0, 0, 0.1)" }}
            whileTap={{ scale: 0.98 }}
            className={`bg-white/10 backdrop-blur-sm rounded-lg p-4 border transition-all duration-200 hover:bg-white/15 relative group ${device.is_connected
                ? 'border-green-500/50 shadow-lg shadow-green-500/10'
                : device.is_paired
                    ? 'border-blue-500/50'
                    : device.is_blocked
                        ? 'border-red-500/50'
                        : 'border-white/20'
                } ${isSelected ? 'ring-2 ring-blue-400' : ''} ${isFavorite ? 'ring-1 ring-yellow-400/50' : ''}`}
        >
            {/* Selection checkbox */}
            <div className="absolute top-2 left-2">
                <motion.button
                    whileHover={{ scale: 1.1 }}
                    whileTap={{ scale: 0.9 }}
                    onClick={onSelect}
                    className="opacity-0 group-hover:opacity-100 transition-opacity"
                >
                    {isSelected ? (
                        <CheckSquare className="w-4 h-4 text-blue-400" />
                    ) : (
                        <Square className="w-4 h-4 text-gray-400" />
                    )}
                </motion.button>
            </div>

            {/* Favorite indicator */}
            {isFavorite && (
                <motion.div
                    initial={{ scale: 0 }}
                    animate={{ scale: 1 }}
                    className="absolute top-2 left-8"
                >
                    <Star className="w-4 h-4 text-yellow-400 fill-current" />
                </motion.div>
            )}

            {/* Device menu */}
            <div className="absolute top-2 right-2">
                <motion.button
                    whileHover={{ scale: 1.1 }}
                    whileTap={{ scale: 0.9 }}
                    onClick={() => setDeviceMenuOpen(!deviceMenuOpen)}
                    className="opacity-0 group-hover:opacity-100 transition-opacity p-1 hover:bg-white/10 rounded"
                >
                    <MoreVertical className="w-4 h-4 text-gray-400" />
                </motion.button>

                <AnimatePresence>
                    {deviceMenuOpen && (
                        <motion.div
                            initial={{ opacity: 0, scale: 0.95, y: -10 }}
                            animate={{ opacity: 1, scale: 1, y: 0 }}
                            exit={{ opacity: 0, scale: 0.95, y: -10 }}
                            className="absolute right-0 top-8 bg-black/90 border border-white/20 rounded-lg py-1 min-w-[150px] z-10"
                        >
                            <button
                                onClick={() => {
                                    setShowDeviceDetails(true);
                                    setDeviceMenuOpen(false);
                                }}
                                className="w-full px-3 py-2 text-left text-sm text-gray-300 hover:bg-white/10 flex items-center space-x-2"
                            >
                                <Eye className="w-4 h-4" />
                                <span>View Details</span>
                            </button>

                            {!device.is_paired && !device.is_blocked && (
                                <button
                                    onClick={() => {
                                        onPair();
                                        setDeviceMenuOpen(false);
                                    }}
                                    className="w-full px-3 py-2 text-left text-sm text-gray-300 hover:bg-white/10 flex items-center space-x-2"
                                >
                                    <Shield className="w-4 h-4" />
                                    <span>Pair Device</span>
                                </button>
                            )}

                            {device.is_paired && (
                                <button
                                    onClick={() => {
                                        onUnpair();
                                        setDeviceMenuOpen(false);
                                    }}
                                    className="w-full px-3 py-2 text-left text-sm text-gray-300 hover:bg-white/10 flex items-center space-x-2"
                                >
                                    <UserX className="w-4 h-4" />
                                    <span>Unpair</span>
                                </button>
                            )}

                            {!device.is_blocked ? (
                                <button
                                    onClick={() => {
                                        onBlock();
                                        setDeviceMenuOpen(false);
                                    }}
                                    className="w-full px-3 py-2 text-left text-sm text-red-400 hover:bg-red-500/10 flex items-center space-x-2"
                                >
                                    <Ban className="w-4 h-4" />
                                    <span>Block Device</span>
                                </button>
                            ) : (
                                <button
                                    onClick={() => {
                                        onUnblock();
                                        setDeviceMenuOpen(false);
                                    }}
                                    className="w-full px-3 py-2 text-left text-sm text-green-400 hover:bg-green-500/10 flex items-center space-x-2"
                                >
                                    <Check className="w-4 h-4" />
                                    <span>Unblock</span>
                                </button>
                            )}

                            <button
                                onClick={() => {
                                    onForget();
                                    setDeviceMenuOpen(false);
                                }}
                                className="w-full px-3 py-2 text-left text-sm text-red-400 hover:bg-red-500/10 flex items-center space-x-2"
                            >
                                <Trash2 className="w-4 h-4" />
                                <span>Forget Device</span>
                            </button>
                        </motion.div>
                    )}
                </AnimatePresence>
            </div>

            <div className="flex items-center justify-between mb-3 mt-4">
                <div className="flex items-center space-x-3">
                    <motion.div
                        whileHover={{ scale: 1.1 }}
                        transition={{ type: "spring", stiffness: 400, damping: 10 }}
                    >
                        {getDeviceIcon(device.device_type, device.platform, device.is_connected)}
                    </motion.div>
                    <div className="flex-1">
                        {editingDevice ? (
                            <div className="flex items-center space-x-2">
                                <input
                                    type="text"
                                    value={newDeviceName}
                                    onChange={(e) => setNewDeviceName(e.target.value)}
                                    className="bg-black/20 text-white text-sm px-2 py-1 rounded border border-white/30 focus:outline-none focus:border-blue-400 w-32"
                                    placeholder={device.name}
                                    autoFocus
                                    maxLength={50}
                                />
                                <motion.button
                                    whileHover={{ scale: 1.1 }}
                                    whileTap={{ scale: 0.9 }}
                                    onClick={handleRename}
                                    className="text-green-400 hover:text-green-300"
                                >
                                    <Check className="w-4 h-4" />
                                </motion.button>
                                <motion.button
                                    whileHover={{ scale: 1.1 }}
                                    whileTap={{ scale: 0.9 }}
                                    onClick={cancelEditing}
                                    className="text-red-400 hover:text-red-300"
                                >
                                    <X className="w-4 h-4" />
                                </motion.button>
                            </div>
                        ) : (
                            <div className="flex items-center space-x-2">
                                <h3 className="text-white font-medium text-sm">
                                    {device.display_name || device.name}
                                </h3>
                                {device.display_name && (
                                    <span className="text-xs text-gray-500">({device.name})</span>
                                )}
                                <motion.button
                                    whileHover={{ scale: 1.1 }}
                                    whileTap={{ scale: 0.9 }}
                                    onClick={startEditing}
                                    className="text-gray-400 hover:text-white opacity-0 group-hover:opacity-100 transition-opacity"
                                >
                                    <Edit2 className="w-3 h-3" />
                                </motion.button>
                            </div>
                        )}
                        <div className="flex items-center space-x-2 mt-1">
                            {device.platform && (
                                <span className="text-xs text-gray-400">{device.platform}</span>
                            )}
                            <span className="text-xs text-gray-500">•</span>
                            <span className="text-xs text-gray-400">{device.address}</span>
                            <span className="text-xs text-gray-500">•</span>
                            <span className="text-xs text-gray-400">{getTimeAgo(device.last_seen)}</span>
                        </div>
                    </div>
                </div>
                <div className="flex items-center space-x-2">
                    <motion.div
                        whileHover={{ scale: 1.1 }}
                        transition={{ type: "spring", stiffness: 400, damping: 10 }}
                    >
                        {getTrustLevelIndicator(device.trust_level)}
                    </motion.div>
                    {device.is_connected ? (
                        <motion.div
                            initial={{ scale: 0 }}
                            animate={{ scale: 1 }}
                            className="flex items-center space-x-1"
                        >
                            <CheckCircle className="w-4 h-4 text-green-500" />
                            <span className="text-xs text-green-400">Connected</span>
                        </motion.div>
                    ) : device.is_blocked ? (
                        <motion.div
                            initial={{ scale: 0 }}
                            animate={{ scale: 1 }}
                            className="flex items-center space-x-1"
                        >
                            <Ban className="w-4 h-4 text-red-500" />
                            <span className="text-xs text-red-400">Blocked</span>
                        </motion.div>
                    ) : device.is_paired ? (
                        <motion.div
                            initial={{ scale: 0 }}
                            animate={{ scale: 1 }}
                            className="flex items-center space-x-1"
                        >
                            <Clock className="w-4 h-4 text-blue-500" />
                            <span className="text-xs text-blue-400">Paired</span>
                        </motion.div>
                    ) : (
                        <motion.div
                            initial={{ scale: 0 }}
                            animate={{ scale: 1 }}
                            className="flex items-center space-x-1"
                        >
                            <AlertCircle className="w-4 h-4 text-gray-500" />
                            <span className="text-xs text-gray-400">Available</span>
                        </motion.div>
                    )}
                </div>
            </div>

            <div className="flex items-center justify-between">
                <div className="flex items-center space-x-3 text-xs text-gray-400">
                    <div className="flex items-center space-x-1">
                        <Globe className="w-3 h-3" />
                        <span>{device.version}</span>
                    </div>
                    {device.connection_count > 0 && (
                        <div className="flex items-center space-x-1">
                            <Wifi className="w-3 h-3" />
                            <span>{device.connection_count} connections</span>
                        </div>
                    )}
                    {device.total_transfers > 0 && (
                        <div className="flex items-center space-x-1">
                            <Star className="w-3 h-3" />
                            <span>{device.total_transfers} transfers</span>
                        </div>
                    )}
                </div>

                <div className="flex space-x-2">
                    {!device.is_blocked && !device.is_paired && (
                        <motion.button
                            whileHover={{ scale: 1.05 }}
                            whileTap={{ scale: 0.95 }}
                            onClick={onPair}
                            className="text-xs px-3 py-1 bg-blue-500/20 text-blue-300 rounded hover:bg-blue-500/30 transition-colors"
                        >
                            Pair
                        </motion.button>
                    )}
                    {device.is_paired && (
                        <motion.button
                            whileHover={{ scale: 1.05 }}
                            whileTap={{ scale: 0.95 }}
                            onClick={onUnpair}
                            className="text-xs px-3 py-1 bg-yellow-500/20 text-yellow-300 rounded hover:bg-yellow-500/30 transition-colors"
                        >
                            Unpair
                        </motion.button>
                    )}
                </div>
            </div>

            {/* Device Management Section */}
            <motion.div
                initial={{ height: 0, opacity: 0 }}
                animate={{ height: 'auto', opacity: 1 }}
                transition={{ delay: 0.1 }}
                className="mt-3 pt-3 border-t border-white/10"
            >
                <DeviceManagement
                    deviceId={device.id}
                    deviceName={device.display_name || device.name}
                    isFavorite={isFavorite}
                    onToggleFavorite={onToggleFavorite}
                    onConnectDevice={onConnect}
                    onDisconnectDevice={onDisconnect}
                    isConnected={device.is_connected}
                    stats={undefined} // You can pass actual stats here if available
                />
            </motion.div>

            {/* Device Details Modal */}
            <AnimatePresence>
                {showDeviceDetails && (
                    <motion.div
                        initial={{ opacity: 0 }}
                        animate={{ opacity: 1 }}
                        exit={{ opacity: 0 }}
                        className="fixed inset-0 bg-black/50 flex items-center justify-center z-50"
                        onClick={() => setShowDeviceDetails(false)}
                    >
                        <motion.div
                            initial={{ opacity: 0, scale: 0.8, y: 20 }}
                            animate={{ opacity: 1, scale: 1, y: 0 }}
                            exit={{ opacity: 0, scale: 0.8, y: 20 }}
                            className="bg-slate-800 rounded-lg border border-white/20 p-6 max-w-md w-full mx-4 max-h-[80vh] overflow-y-auto"
                            onClick={(e) => e.stopPropagation()}
                        >
                            <div className="flex items-center justify-between mb-4">
                                <h3 className="text-white font-semibold">Device Details</h3>
                                <motion.button
                                    whileHover={{ scale: 1.1 }}
                                    whileTap={{ scale: 0.9 }}
                                    onClick={() => setShowDeviceDetails(false)}
                                    className="text-gray-400 hover:text-white"
                                >
                                    <X className="w-5 h-5" />
                                </motion.button>
                            </div>

                            <div className="space-y-4">
                                <div>
                                    <label className="text-sm text-gray-400">Name</label>
                                    <p className="text-white">{device.display_name || device.name}</p>
                                    {device.display_name && (
                                        <p className="text-xs text-gray-500">Original: {device.name}</p>
                                    )}
                                </div>

                                <div>
                                    <label className="text-sm text-gray-400">Device Type</label>
                                    <div className="flex items-center space-x-2">
                                        {getDeviceIcon(device.device_type, device.platform, device.is_connected)}
                                        <span className="text-white capitalize">{device.device_type}</span>
                                        {device.platform && (
                                            <span className="text-gray-400">({device.platform})</span>
                                        )}
                                    </div>
                                </div>

                                <div>
                                    <label className="text-sm text-gray-400">Status</label>
                                    <div className="flex items-center space-x-2">
                                        {getTrustLevelIndicator(device.trust_level)}
                                        <span className="text-white">{device.trust_level}</span>
                                    </div>
                                </div>

                                <div>
                                    <label className="text-sm text-gray-400">Address</label>
                                    <p className="text-white font-mono text-sm">{device.address}</p>
                                </div>

                                <div>
                                    <label className="text-sm text-gray-400">Version</label>
                                    <p className="text-white">{device.version}</p>
                                </div>

                                <div className="grid grid-cols-2 gap-4">
                                    <div>
                                        <label className="text-sm text-gray-400">First Seen</label>
                                        <p className="text-white text-sm">{new Date(device.first_seen * 1000).toLocaleDateString()}</p>
                                    </div>
                                    <div>
                                        <label className="text-sm text-gray-400">Last Seen</label>
                                        <p className="text-white text-sm">{getTimeAgo(device.last_seen)}</p>
                                    </div>
                                </div>

                                <div className="grid grid-cols-2 gap-4">
                                    <div>
                                        <label className="text-sm text-gray-400">Connections</label>
                                        <p className="text-white">{device.connection_count}</p>
                                    </div>
                                    <div>
                                        <label className="text-sm text-gray-400">Transfers</label>
                                        <p className="text-white">{device.total_transfers}</p>
                                    </div>
                                </div>

                                {device.last_transfer_time && (
                                    <div>
                                        <label className="text-sm text-gray-400">Last Transfer</label>
                                        <p className="text-white text-sm">{getTimeAgo(device.last_transfer_time)}</p>
                                    </div>
                                )}
                            </div>
                        </motion.div>
                    </motion.div>
                )}
            </AnimatePresence>
        </motion.div>
    );
};

export default DeviceCard;