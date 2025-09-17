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
    Eye,
    Ban,
    CheckSquare,
    Square,
    Star,
    Wifi,
    AlertTriangle
} from 'lucide-react';
import { useTheme } from '../context/ThemeContext';

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
    onBlock: () => void;
    onUnblock: () => void;
    onForget: () => void;
    onRename: (newName: string) => void;
    onToggleFavorite: () => void;
}

const DeviceCard: React.FC<DeviceCardProps> = ({
    device,
    isSelected,
    isFavorite,
    onSelect,
    onPair,
    onBlock,
    onUnblock,
    onForget,
    onRename,
    onToggleFavorite
}) => {
    const { theme } = useTheme();
    const [editingDevice, setEditingDevice] = useState(false);
    const [newDeviceName, setNewDeviceName] = useState('');
    const [deviceMenuOpen, setDeviceMenuOpen] = useState(false);
    const [showDeviceDetails, setShowDeviceDetails] = useState(false);

    // Device icon helper
    const getDeviceIcon = (deviceType: string, _platform: string | undefined, isConnected: boolean) => {
        const iconClass = `w-6 h-6`;
        const iconStyle = { color: isConnected ? theme.colors.success : theme.colors.textTertiary };

        switch (deviceType.toLowerCase()) {
            case 'phone':
            case 'mobile':
                return <Smartphone className={iconClass} style={iconStyle} />;
            case 'tablet':
                return <Tablet className={iconClass} style={iconStyle} />;
            case 'laptop':
                return <Laptop className={iconClass} style={iconStyle} />;
            case 'server':
                return <Server className={iconClass} style={iconStyle} />;
            case 'desktop':
            default:
                return <Monitor className={iconClass} style={iconStyle} />;
        }
    };

    // Trust level indicator
    const getTrustLevelIndicator = (trustLevel: string) => {
        switch (trustLevel) {
            case 'Verified':
                return <ShieldCheck className="w-4 h-4" style={{ color: theme.colors.success }} />;
            case 'Trusted':
                return <Shield className="w-4 h-4" style={{ color: theme.colors.info }} />;
            case 'Blocked':
                return <ShieldX className="w-4 h-4" style={{ color: theme.colors.error }} />;
            default:
                return <AlertTriangle className="w-4 h-4" style={{ color: theme.colors.warning }} />;
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
            whileHover={{ y: -2, boxShadow: `0 10px 25px -5px ${theme.colors.shadow}` }}
            whileTap={{ scale: 0.98 }}
            className="backdrop-blur-sm rounded-lg p-4 transition-all duration-200 relative group"
            style={{
                backgroundColor: theme.colors.backgroundSecondary + '80',
                border: `1px solid ${
                    device.is_connected ? theme.colors.success + '80'
                    : device.is_paired ? theme.colors.info + '80'
                    : device.is_blocked ? theme.colors.error + '80'
                    : theme.colors.border
                }`,
                boxShadow: device.is_connected ? `0 0 20px ${theme.colors.success}20` : 'none',
                outline: isSelected ? `2px solid ${theme.colors.accent2}` : 'none',
                outlineOffset: '2px',
                borderColor: isFavorite ? theme.colors.warning + '80' : undefined
            }}
            onMouseEnter={(e) => {
                e.currentTarget.style.backgroundColor = theme.colors.backgroundTertiary + '90';
                e.currentTarget.style.transform = 'translateY(-2px)';
            }}
            onMouseLeave={(e) => {
                e.currentTarget.style.backgroundColor = theme.colors.backgroundSecondary + '80';
                e.currentTarget.style.transform = 'translateY(0)';
            }}
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
                        <CheckSquare className="w-4 h-4" style={{ color: theme.colors.accent2 }} />
                    ) : (
                        <Square className="w-4 h-4" style={{ color: theme.colors.textTertiary }} />
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
                    <Star className="w-4 h-4 fill-current" style={{ color: theme.colors.warning }} />
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
                    <MoreVertical className="w-4 h-4" style={{ color: theme.colors.textTertiary }} />
                </motion.button>

                <AnimatePresence>
                    {deviceMenuOpen && (
                        <motion.div
                            initial={{ opacity: 0, scale: 0.95, y: -10 }}
                            animate={{ opacity: 1, scale: 1, y: 0 }}
                            exit={{ opacity: 0, scale: 0.95, y: -10 }}
                            className="absolute right-0 top-8 backdrop-blur-md rounded-lg py-1 min-w-[150px] z-10"
                            style={{
                                backgroundColor: theme.colors.background + 'F5',
                                border: `1px solid ${theme.colors.border}`
                            }}
                        >
                            <button
                                onClick={() => {
                                    setShowDeviceDetails(true);
                                    setDeviceMenuOpen(false);
                                }}
                                className="w-full px-3 py-2 text-left text-sm flex items-center space-x-2 transition-colors"
                                style={{ color: theme.colors.textSecondary }}
                                onMouseEnter={(e) => {
                                    e.currentTarget.style.backgroundColor = theme.colors.backgroundSecondary;
                                    e.currentTarget.style.color = theme.colors.text;
                                }}
                                onMouseLeave={(e) => {
                                    e.currentTarget.style.backgroundColor = 'transparent';
                                    e.currentTarget.style.color = theme.colors.textSecondary;
                                }}
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
                                    className="w-full px-3 py-2 text-left text-sm flex items-center space-x-2 transition-colors"
                                style={{ color: theme.colors.textSecondary }}
                                onMouseEnter={(e) => {
                                    e.currentTarget.style.backgroundColor = theme.colors.backgroundSecondary;
                                    e.currentTarget.style.color = theme.colors.text;
                                }}
                                onMouseLeave={(e) => {
                                    e.currentTarget.style.backgroundColor = 'transparent';
                                    e.currentTarget.style.color = theme.colors.textSecondary;
                                }}
                                >
                                    <Shield className="w-4 h-4" />
                                    <span>Pair Device</span>
                                </button>
                            )}


                            {!device.is_blocked ? (
                                <button
                                    onClick={() => {
                                        onBlock();
                                        setDeviceMenuOpen(false);
                                    }}
                                    className="w-full px-3 py-2 text-left text-sm flex items-center space-x-2 transition-colors"
                                    style={{ color: theme.colors.error }}
                                    onMouseEnter={(e) => {
                                        e.currentTarget.style.backgroundColor = theme.colors.error + '20';
                                    }}
                                    onMouseLeave={(e) => {
                                        e.currentTarget.style.backgroundColor = 'transparent';
                                    }}
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
                                    className="w-full px-3 py-2 text-left text-sm flex items-center space-x-2 transition-colors"
                                    style={{ color: theme.colors.success }}
                                    onMouseEnter={(e) => {
                                        e.currentTarget.style.backgroundColor = theme.colors.success + '20';
                                    }}
                                    onMouseLeave={(e) => {
                                        e.currentTarget.style.backgroundColor = 'transparent';
                                    }}
                                >
                                    <Check className="w-4 h-4" />
                                    <span>Unblock</span>
                                </button>
                            )}

                            <button
                                onClick={() => {
                                    onToggleFavorite();
                                    setDeviceMenuOpen(false);
                                }}
                                className="w-full px-3 py-2 text-left text-sm flex items-center space-x-2 transition-colors"
                                style={{ color: theme.colors.textSecondary }}
                                onMouseEnter={(e) => {
                                    e.currentTarget.style.backgroundColor = theme.colors.backgroundSecondary;
                                    e.currentTarget.style.color = theme.colors.text;
                                }}
                                onMouseLeave={(e) => {
                                    e.currentTarget.style.backgroundColor = 'transparent';
                                    e.currentTarget.style.color = theme.colors.textSecondary;
                                }}
                            >
                                <Star className="w-4 h-4" />
                                <span>{isFavorite ? 'Remove from Favorites' : 'Add to Favorites'}</span>
                            </button>

                            <button
                                onClick={() => {
                                    onForget();
                                    setDeviceMenuOpen(false);
                                }}
                                className="w-full px-3 py-2 text-left text-sm text-red-400 hover:bg-red-500/10 flex items-center space-x-2"
                            >
                                <Trash2 className="w-4 h-4" />
                                <span>Remove Device</span>
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
                                    className="text-sm px-2 py-1 rounded border focus:outline-none w-32"
                                    style={{
                                        backgroundColor: theme.colors.backgroundTertiary,
                                        color: theme.colors.text,
                                        borderColor: theme.colors.border
                                    }}
                                    onFocus={(e) => {
                                        e.currentTarget.style.borderColor = theme.colors.accent2;
                                    }}
                                    onBlur={(e) => {
                                        e.currentTarget.style.borderColor = theme.colors.border;
                                    }}
                                    placeholder={device.name}
                                    autoFocus
                                    maxLength={50}
                                />
                                <motion.button
                                    whileHover={{ scale: 1.1 }}
                                    whileTap={{ scale: 0.9 }}
                                    onClick={handleRename}
                                    style={{ color: theme.colors.success }}
                                    onMouseEnter={(e) => {
                                        e.currentTarget.style.color = theme.colors.accent1;
                                    }}
                                    onMouseLeave={(e) => {
                                        e.currentTarget.style.color = theme.colors.success;
                                    }}
                                >
                                    <Check className="w-4 h-4" />
                                </motion.button>
                                <motion.button
                                    whileHover={{ scale: 1.1 }}
                                    whileTap={{ scale: 0.9 }}
                                    onClick={cancelEditing}
                                    style={{ color: theme.colors.error }}
                                    onMouseEnter={(e) => {
                                        e.currentTarget.style.color = theme.colors.hover;
                                    }}
                                    onMouseLeave={(e) => {
                                        e.currentTarget.style.color = theme.colors.error;
                                    }}
                                >
                                    <X className="w-4 h-4" />
                                </motion.button>
                            </div>
                        ) : (
                            <div className="flex items-center space-x-2">
                                <h3 className="font-medium text-sm" style={{ color: theme.colors.text }}>
                                    {device.display_name || device.name}
                                </h3>
                                {device.display_name && (
                                    <span className="text-xs" style={{ color: theme.colors.textTertiary }}>({device.name})</span>
                                )}
                                <motion.button
                                    whileHover={{ scale: 1.1 }}
                                    whileTap={{ scale: 0.9 }}
                                    onClick={startEditing}
                                    className="opacity-0 group-hover:opacity-100 transition-opacity"
                                    style={{ color: theme.colors.textSecondary }}
                                    onMouseEnter={(e) => {
                                        e.currentTarget.style.color = theme.colors.text;
                                    }}
                                    onMouseLeave={(e) => {
                                        e.currentTarget.style.color = theme.colors.textSecondary;
                                    }}
                                >
                                    <Edit2 className="w-3 h-3" />
                                </motion.button>
                            </div>
                        )}
                        <div className="flex items-center space-x-2 mt-1">
                            {device.platform && (
                                <>
                                    <span className="text-xs" style={{ color: theme.colors.textSecondary }}>{device.platform}</span>
                                    <span className="text-xs" style={{ color: theme.colors.textTertiary }}>•</span>
                                </>
                            )}
                            <span className="text-xs" style={{ color: theme.colors.textSecondary }}>{device.address}</span>
                            <span className="text-xs" style={{ color: theme.colors.textTertiary }}>•</span>
                            <span className="text-xs" style={{ color: theme.colors.textSecondary }}>Last online: {getTimeAgo(device.last_seen)}</span>
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
                            <CheckCircle className="w-4 h-4" style={{ color: theme.colors.success }} />
                            <span className="text-xs" style={{ color: theme.colors.success }}>Connected</span>
                        </motion.div>
                    ) : device.is_blocked ? (
                        <motion.div
                            initial={{ scale: 0 }}
                            animate={{ scale: 1 }}
                            className="flex items-center space-x-1"
                        >
                            <Ban className="w-4 h-4" style={{ color: theme.colors.error }} />
                            <span className="text-xs" style={{ color: theme.colors.error }}>Blocked</span>
                        </motion.div>
                    ) : device.is_paired ? (
                        <motion.div
                            initial={{ scale: 0 }}
                            animate={{ scale: 1 }}
                            className="flex items-center space-x-1"
                        >
                            <Clock className="w-4 h-4" style={{ color: theme.colors.info }} />
                            <span className="text-xs" style={{ color: theme.colors.info }}>Paired</span>
                        </motion.div>
                    ) : (
                        <motion.div
                            initial={{ scale: 0 }}
                            animate={{ scale: 1 }}
                            className="flex items-center space-x-1"
                        >
                            <AlertCircle className="w-4 h-4" style={{ color: theme.colors.textTertiary }} />
                            <span className="text-xs" style={{ color: theme.colors.textSecondary }}>Available</span>
                        </motion.div>
                    )}
                </div>
            </div>

            <div className="flex items-center justify-between">
                <div className="flex items-center space-x-3 text-xs" style={{ color: theme.colors.textSecondary }}>
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
                            className="text-xs px-3 py-1 rounded transition-all duration-200"
                            style={{
                                backgroundColor: theme.colors.accent2 + '20',
                                color: theme.colors.accent2,
                                border: `1px solid ${theme.colors.accent2}40`
                            }}
                            onMouseEnter={(e) => {
                                e.currentTarget.style.backgroundColor = theme.colors.accent2 + '30';
                                e.currentTarget.style.boxShadow = `0 0 12px ${theme.colors.accent2}40`;
                            }}
                            onMouseLeave={(e) => {
                                e.currentTarget.style.backgroundColor = theme.colors.accent2 + '20';
                                e.currentTarget.style.boxShadow = 'none';
                            }}
                        >
                            Pair
                        </motion.button>
                    )}
                </div>
            </div>


            {/* Device Details Modal */}
            <AnimatePresence>
                {showDeviceDetails && (
                    <motion.div
                        initial={{ opacity: 0 }}
                        animate={{ opacity: 1 }}
                        exit={{ opacity: 0 }}
                        className="fixed inset-0 flex items-center justify-center z-50"
                        style={{ backgroundColor: theme.colors.background + '80' }}
                        onClick={() => setShowDeviceDetails(false)}
                    >
                        <motion.div
                            initial={{ opacity: 0, scale: 0.8, y: 20 }}
                            animate={{ opacity: 1, scale: 1, y: 0 }}
                            exit={{ opacity: 0, scale: 0.8, y: 20 }}
                            className="rounded-lg border p-6 max-w-md w-full mx-4 max-h-[80vh] overflow-y-auto backdrop-blur-md"
                            style={{
                                backgroundColor: theme.colors.backgroundSecondary + 'F5',
                                border: `1px solid ${theme.colors.border}`,
                                boxShadow: `0 20px 50px ${theme.colors.shadow}`
                            }}
                            onClick={(e) => e.stopPropagation()}
                        >
                            <div className="flex items-center justify-between mb-4">
                                <h3 className="font-semibold" style={{ color: theme.colors.text }}>Device Details</h3>
                                <motion.button
                                    whileHover={{ scale: 1.1 }}
                                    whileTap={{ scale: 0.9 }}
                                    onClick={() => setShowDeviceDetails(false)}
                                    style={{ color: theme.colors.textSecondary }}
                                    onMouseEnter={(e) => {
                                        e.currentTarget.style.color = theme.colors.text;
                                    }}
                                    onMouseLeave={(e) => {
                                        e.currentTarget.style.color = theme.colors.textSecondary;
                                    }}
                                >
                                    <X className="w-5 h-5" />
                                </motion.button>
                            </div>

                            <div className="space-y-4">
                                <div>
                                    <label className="text-sm" style={{ color: theme.colors.textSecondary }}>Name</label>
                                    <p style={{ color: theme.colors.text }}>{device.display_name || device.name}</p>
                                    {device.display_name && (
                                        <p className="text-xs" style={{ color: theme.colors.textTertiary }}>Original: {device.name}</p>
                                    )}
                                </div>

                                <div>
                                    <label className="text-sm" style={{ color: theme.colors.textSecondary }}>Device Type</label>
                                    <div className="flex items-center space-x-2">
                                        {getDeviceIcon(device.device_type, device.platform, device.is_connected)}
                                        <span style={{ color: theme.colors.text }} className=" capitalize">{device.device_type}</span>
                                        {device.platform && (
                                            <span style={{ color: theme.colors.textSecondary }}>({device.platform})</span>
                                        )}
                                    </div>
                                </div>

                                <div>
                                    <label className="text-sm" style={{ color: theme.colors.textSecondary }}>Status</label>
                                    <div className="flex items-center space-x-2">
                                        {getTrustLevelIndicator(device.trust_level)}
                                        <span style={{ color: theme.colors.text }} className="">{device.trust_level}</span>
                                    </div>
                                </div>

                                <div>
                                    <label className="text-sm" style={{ color: theme.colors.textSecondary }}>Address</label>
                                    <p className="font-mono text-sm" style={{ color: theme.colors.text }}>{device.address}</p>
                                </div>

                                <div>
                                    <label className="text-sm" style={{ color: theme.colors.textSecondary }}>Version</label>
                                    <p style={{ color: theme.colors.text }}>{device.version}</p>
                                </div>

                                <div className="grid grid-cols-2 gap-4">
                                    <div>
                                        <label className="text-sm" style={{ color: theme.colors.textSecondary }}>First Seen</label>
                                        <p className="text-sm" style={{ color: theme.colors.text }}>{new Date(device.first_seen * 1000).toLocaleDateString()}</p>
                                    </div>
                                    <div>
                                        <label className="text-sm" style={{ color: theme.colors.textSecondary }}>Last Seen</label>
                                        <p className="text-sm" style={{ color: theme.colors.text }}>{getTimeAgo(device.last_seen)}</p>
                                    </div>
                                </div>

                                <div className="grid grid-cols-2 gap-4">
                                    <div>
                                        <label className="text-sm" style={{ color: theme.colors.textSecondary }}>Connections</label>
                                        <p style={{ color: theme.colors.text }}>{device.connection_count}</p>
                                    </div>
                                    <div>
                                        <label className="text-sm" style={{ color: theme.colors.textSecondary }}>Transfers</label>
                                        <p style={{ color: theme.colors.text }}>{device.total_transfers}</p>
                                    </div>
                                </div>

                                {device.last_transfer_time && (
                                    <div>
                                        <label className="text-sm" style={{ color: theme.colors.textSecondary }}>Last Transfer</label>
                                        <p className="text-sm" style={{ color: theme.colors.text }}>{getTimeAgo(device.last_transfer_time)}</p>
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