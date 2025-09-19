import React, { useState, useEffect, useRef } from 'react';
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
    MoreVertical,
    Trash2,
    Ban
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
    const dropdownRef = useRef<HTMLDivElement>(null);

    // Handle click outside to close dropdown
    useEffect(() => {
        const handleClickOutside = (event: MouseEvent) => {
            if (dropdownRef.current && !dropdownRef.current.contains(event.target as Node)) {
                setDeviceMenuOpen(false);
            }
        };

        if (deviceMenuOpen) {
            document.addEventListener('mousedown', handleClickOutside);
        }

        return () => {
            document.removeEventListener('mousedown', handleClickOutside);
        };
    }, [deviceMenuOpen]);

    // Time formatting helper
    const getTimeAgo = (timestamp: number) => {
        const now = Math.floor(Date.now() / 1000);
        const diff = now - timestamp;

        if (diff < 60) return 'Just now';
        if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
        if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
        return `${Math.floor(diff / 86400)}d ago`;
    };

    // Device icon helper
    const getDeviceIcon = (deviceType: string, _platform: string | undefined, _isConnected: boolean) => {
        const iconClass = `w-5 h-5`;
        const iconStyle = { color: theme.colors.textSecondary };

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
            whileHover={{ y: -1 }}
            className="backdrop-blur-sm rounded-lg p-3 transition-all duration-200 group"
            style={{
                backgroundColor: theme.colors.backgroundSecondary + '80',
                border: `1px solid ${theme.colors.border}`,
                outline: isSelected ? `2px solid ${theme.colors.accent2}` : 'none',
                outlineOffset: '2px'
            }}
            onMouseEnter={(e) => {
                e.currentTarget.style.backgroundColor = theme.colors.backgroundTertiary + '90';
            }}
            onMouseLeave={(e) => {
                e.currentTarget.style.backgroundColor = theme.colors.backgroundSecondary + '80';
            }}
        >
            <div className="flex items-center justify-between">
                {/* Left side - Device info */}
                <div className="flex items-center space-x-3 flex-1">
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
                            <div>
                                <div className="flex items-center space-x-2">
                                    <h3 className="font-medium text-sm" style={{ color: theme.colors.text }}>
                                        {device.display_name || device.name}
                                    </h3>
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
                                <div className="flex items-center space-x-2 mt-1">
                                    <span className="text-xs" style={{ color: theme.colors.textTertiary }}>
                                        {device.platform || device.device_type || 'Device'} · {device.address !== 'Unknown' ? device.address : 'Offline'} · {device.last_seen ? getTimeAgo(device.last_seen) : 'Never'}
                                    </span>
                                </div>
                            </div>
                        )}
                    </div>
                </div>

                {/* Right side - Status pill and More button */}
                <div className="flex items-center space-x-2">
                    {/* Online/Offline status pill */}
                    {device.is_connected ? (
                        <motion.div
                            initial={{ scale: 0 }}
                            animate={{ scale: 1 }}
                            className="flex items-center space-x-1 px-2 py-1 rounded-full"
                            style={{
                                backgroundColor: theme.colors.success + '20',
                                border: `1px solid ${theme.colors.success}40`
                            }}
                        >
                            <div className="w-2 h-2 rounded-full" style={{ backgroundColor: theme.colors.success }} />
                            <span className="text-xs font-medium" style={{ color: theme.colors.success }}>Online</span>
                        </motion.div>
                    ) : (
                        <motion.div
                            initial={{ scale: 0 }}
                            animate={{ scale: 1 }}
                            className="flex items-center space-x-1 px-2 py-1 rounded-full"
                            style={{
                                backgroundColor: theme.colors.error + '20',
                                border: `1px solid ${theme.colors.error}40`
                            }}
                        >
                            <div className="w-2 h-2 rounded-full" style={{ backgroundColor: theme.colors.error }} />
                            <span className="text-xs font-medium" style={{ color: theme.colors.error }}>Offline</span>
                        </motion.div>
                    )}

                    {/* More button with dropdown menu */}
                    <div className="relative" ref={dropdownRef}>
                        <motion.button
                            whileHover={{ scale: 1.05 }}
                            whileTap={{ scale: 0.95 }}
                            onClick={() => setDeviceMenuOpen(!deviceMenuOpen)}
                            className="flex items-center space-x-1 px-2 py-1 rounded-full transition-all duration-200"
                            style={{
                                backgroundColor: theme.colors.backgroundTertiary,
                                border: `1px solid ${theme.colors.border}`,
                                color: theme.colors.textSecondary
                            }}
                            onMouseEnter={(e) => {
                                e.currentTarget.style.backgroundColor = theme.colors.backgroundSecondary;
                                e.currentTarget.style.borderColor = theme.colors.accent2;
                            }}
                            onMouseLeave={(e) => {
                                e.currentTarget.style.backgroundColor = theme.colors.backgroundTertiary;
                                e.currentTarget.style.borderColor = theme.colors.border;
                            }}
                        >
                            <MoreVertical className="w-3 h-3" />
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
                                        border: `1px solid ${theme.colors.border}`,
                                        boxShadow: `0 10px 25px -5px ${theme.colors.shadow}`
                                    }}
                                >
                                    {!device.is_blocked ? (
                                        <button
                                            onClick={() => {
                                                onBlock();
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
                                            <Ban className="w-4 h-4" />
                                            <span>Block</span>
                                        </button>
                                    ) : (
                                        <button
                                            onClick={() => {
                                                onUnblock();
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
                                            <Check className="w-4 h-4" />
                                            <span>Unblock</span>
                                        </button>
                                    )}

                                    <button
                                        onClick={() => {
                                            onForget();
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
                                        <Trash2 className="w-4 h-4" />
                                        <span>Remove Device</span>
                                    </button>
                                </motion.div>
                            )}
                        </AnimatePresence>
                    </div>
                </div>
            </div>
        </motion.div>
    );
};

export default DeviceCard;