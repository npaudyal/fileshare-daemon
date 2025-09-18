import React from 'react';
import { motion } from 'framer-motion';
import {
    Smartphone,
    Laptop,
    Monitor,
    Tablet,
    Server,
    Loader2,
    CheckCircle,
    XCircle,
    Key
} from 'lucide-react';
import { useTheme } from '../context/ThemeContext';

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
    state: 'Initiated' | 'DisplayingPin' | 'AwaitingApproval' | 'Confirmed' | 'Completed' | { Failed: any } | 'Timeout';
    initiated_by_us: boolean;
    remaining_seconds: number;
}

interface UnpairedDeviceCardProps {
    device: UnpairedDevice;
    session?: PairingSession;
    onPair: (deviceId: string) => void;
    onConfirm: (sessionId: string) => void;
    onReject: (sessionId: string) => void;
}

const UnpairedDeviceCard: React.FC<UnpairedDeviceCardProps> = ({
    device,
    session,
    onPair,
    onConfirm,
    onReject
}) => {
    const { theme } = useTheme();

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
    const getDeviceIcon = (platform?: string) => {
        const iconClass = `w-5 h-5`;
        const iconStyle = { color: theme.colors.textSecondary };
        const platform_lower = platform?.toLowerCase();

        if (platform_lower === 'android' || platform_lower === 'ios') {
            return <Smartphone className={iconClass} style={iconStyle} />;
        } else if (platform_lower === 'windows') {
            return <Monitor className={iconClass} style={iconStyle} />;
        } else if (platform_lower === 'macos' || platform_lower === 'darwin') {
            return <Laptop className={iconClass} style={iconStyle} />;
        } else if (platform_lower === 'linux') {
            return <Server className={iconClass} style={iconStyle} />;
        }
        return <Tablet className={iconClass} style={iconStyle} />;
    };

    // Get pairing button based on session state
    const getPairingButton = () => {
        if (!session) {
            return (
                <motion.button
                    whileHover={{ scale: 1.05 }}
                    whileTap={{ scale: 0.95 }}
                    onClick={() => onPair(device.id)}
                    className="flex items-center space-x-1 px-3 py-1 rounded-full transition-all duration-200"
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
                    <span className="text-xs font-medium">Pair</span>
                </motion.button>
            );
        }

        // Handle different session states
        const state = session.state;
        if (typeof state === 'object' && 'Failed' in state) {
            return (
                <motion.button
                    whileHover={{ scale: 1.05 }}
                    whileTap={{ scale: 0.95 }}
                    onClick={() => onPair(device.id)}
                    className="flex items-center space-x-1 px-3 py-1 rounded-full transition-all duration-200"
                    style={{
                        backgroundColor: theme.colors.error + '20',
                        color: theme.colors.error,
                        border: `1px solid ${theme.colors.error}40`
                    }}
                >
                    <XCircle className="w-3 h-3" />
                    <span className="text-xs font-medium">Retry</span>
                </motion.button>
            );
        }

        switch (state) {
            case 'Initiated':
            case 'Confirmed':
                return (
                    <button
                        disabled
                        className="flex items-center space-x-1 px-3 py-1 rounded-full"
                        style={{
                            backgroundColor: theme.colors.backgroundTertiary,
                            color: theme.colors.textSecondary,
                            border: `1px solid ${theme.colors.border}`
                        }}
                    >
                        <Loader2 className="w-3 h-3 animate-spin" />
                        <span className="text-xs font-medium">Pairing</span>
                    </button>
                );
            case 'DisplayingPin':
                return (
                    <button
                        disabled
                        className="flex items-center space-x-1 px-3 py-1 rounded-full"
                        style={{
                            backgroundColor: theme.colors.accent2 + '20',
                            color: theme.colors.accent2,
                            border: `1px solid ${theme.colors.accent2}40`
                        }}
                    >
                        <Key className="w-3 h-3" />
                        <span className="text-xs font-medium">Pairing</span>
                    </button>
                );
            case 'AwaitingApproval':
                return (
                    <div className="flex items-center space-x-2">
                        <motion.button
                            whileHover={{ scale: 1.05 }}
                            whileTap={{ scale: 0.95 }}
                            onClick={() => onConfirm(session.session_id)}
                            className="px-3 py-1 rounded-full transition-all duration-200"
                            style={{
                                backgroundColor: theme.colors.success + '20',
                                color: theme.colors.success,
                                border: `1px solid ${theme.colors.success}40`
                            }}
                            onMouseEnter={(e) => {
                                e.currentTarget.style.backgroundColor = theme.colors.success + '30';
                            }}
                            onMouseLeave={(e) => {
                                e.currentTarget.style.backgroundColor = theme.colors.success + '20';
                            }}
                        >
                            <span className="text-xs font-medium">Accept</span>
                        </motion.button>
                        <motion.button
                            whileHover={{ scale: 1.05 }}
                            whileTap={{ scale: 0.95 }}
                            onClick={() => onReject(session.session_id)}
                            className="px-3 py-1 rounded-full transition-all duration-200"
                            style={{
                                backgroundColor: theme.colors.backgroundTertiary,
                                color: theme.colors.textSecondary,
                                border: `1px solid ${theme.colors.border}`
                            }}
                            onMouseEnter={(e) => {
                                e.currentTarget.style.backgroundColor = theme.colors.backgroundSecondary;
                            }}
                            onMouseLeave={(e) => {
                                e.currentTarget.style.backgroundColor = theme.colors.backgroundTertiary;
                            }}
                        >
                            <span className="text-xs font-medium">Reject</span>
                        </motion.button>
                    </div>
                );
            case 'Completed':
                return (
                    <button
                        disabled
                        className="flex items-center space-x-1 px-3 py-1 rounded-full"
                        style={{
                            backgroundColor: theme.colors.success + '20',
                            color: theme.colors.success,
                            border: `1px solid ${theme.colors.success}40`
                        }}
                    >
                        <CheckCircle className="w-3 h-3" />
                        <span className="text-xs font-medium">Paired</span>
                    </button>
                );
            default:
                return (
                    <motion.button
                        whileHover={{ scale: 1.05 }}
                        whileTap={{ scale: 0.95 }}
                        onClick={() => onPair(device.id)}
                        className="flex items-center space-x-1 px-3 py-1 rounded-full transition-all duration-200"
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
                        <span className="text-xs font-medium">Pair</span>
                    </motion.button>
                );
        }
    };

    return (
        <motion.div
            layout
            whileHover={{ y: -1 }}
            className="backdrop-blur-sm rounded-lg p-3 transition-all duration-200 group"
            style={{
                backgroundColor: theme.colors.backgroundSecondary + '80',
                border: `1px solid ${theme.colors.border}`
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
                        {getDeviceIcon(device.platform)}
                    </motion.div>
                    <div className="flex-1">
                        <div>
                            <h3 className="font-medium text-sm" style={{ color: theme.colors.text }}>
                                {device.name}
                            </h3>
                            <div className="flex items-center space-x-2 mt-1">
                                <span className="text-xs" style={{ color: theme.colors.textTertiary }}>
                                    {device.platform || 'Device'} · {device.address} · {getTimeAgo(device.last_seen)}
                                </span>
                            </div>
                        </div>
                    </div>
                </div>

                {/* Right side - Pairing button */}
                <div className="flex items-center">
                    {getPairingButton()}
                </div>
            </div>
        </motion.div>
    );
};

export default UnpairedDeviceCard;