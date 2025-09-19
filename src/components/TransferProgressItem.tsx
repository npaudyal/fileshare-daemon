import React, { useState } from 'react';
import { motion } from 'framer-motion';
import {
    Download,
    Upload,
    Pause,
    Play,
    X,
    CheckCircle,
    XCircle,
    Loader2,
    WifiOff
} from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { useTheme } from '../context/ThemeContext';
import { Transfer } from '../stores/viewStore';

interface TransferProgressItemProps {
    transfer: Transfer;
    onUpdate: () => void;
}

const TransferProgressItem: React.FC<TransferProgressItemProps> = ({ transfer, onUpdate }) => {
    const { theme } = useTheme();
    const [hoveredButton, setHoveredButton] = useState<string | null>(null);

    const formatBytes = (bytes: number) => {
        if (bytes === 0) return '0 B';
        const k = 1024;
        const sizes = ['B', 'KB', 'MB', 'GB'];
        const i = Math.floor(Math.log(bytes) / Math.log(k));
        return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
    };

    const formatSpeed = (bytesPerSecond: number) => {
        if (bytesPerSecond === 0) return '0 B/s';
        return formatBytes(bytesPerSecond / 8) + '/s';
    };

    const formatETA = (seconds?: number) => {
        if (!seconds || seconds === 0) return '∞';
        if (seconds < 60) return `${Math.round(seconds)}s`;
        if (seconds < 3600) return `${Math.round(seconds / 60)}m`;
        return `${Math.round(seconds / 3600)}h ${Math.round((seconds % 3600) / 60)}m`;
    };

    const getProgressPercent = () => {
        if (transfer.file_size === 0) return 0;
        return (transfer.transferred_bytes / transfer.file_size) * 100;
    };

    const handlePauseResume = async () => {
        try {
            await invoke('toggle_transfer_pause', { transferId: transfer.id });
            onUpdate();
        } catch (error) {
            console.error('Failed to toggle transfer:', error);
        }
    };

    const handleCancel = async () => {
        try {
            await invoke('cancel_transfer', { transferId: transfer.id });
            onUpdate();
        } catch (error) {
            console.error('Failed to cancel transfer:', error);
        }
    };

    const getStatusIcon = () => {
        switch (transfer.status) {
            case 'Completed':
                return <CheckCircle className="w-5 h-5" style={{ color: theme.colors.success }} />;
            case 'Failed':
            case 'Cancelled':
                return <XCircle className="w-5 h-5" style={{ color: theme.colors.error }} />;
            case 'Connecting':
                return <Loader2 className="w-5 h-5 animate-spin" style={{ color: theme.colors.accent1 }} />;
            case 'Paused':
                return <WifiOff className="w-5 h-5" style={{ color: theme.colors.warning }} />;
            default:
                return transfer.direction === 'Incoming' ? (
                    <Download className="w-5 h-5" style={{ color: theme.colors.accent2 }} />
                ) : (
                    <Upload className="w-5 h-5" style={{ color: theme.colors.success }} />
                );
        }
    };

    const getStatusColor = () => {
        switch (transfer.status) {
            case 'Completed': return theme.colors.success;
            case 'Failed':
            case 'Cancelled': return theme.colors.error;
            case 'Paused': return theme.colors.warning;
            case 'Connecting': return theme.colors.accent1;
            default: return theme.colors.accent2;
        }
    };

    const isActive = transfer.status === 'InProgress' || transfer.status === 'Paused' || transfer.status === 'Connecting';
    const progressPercent = getProgressPercent();

    return (
        <motion.div
            layout
            initial={{ opacity: 0, y: 20 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: -20 }}
            className="rounded-xl p-6 backdrop-blur-md"
            style={{
                backgroundColor: theme.colors.backgroundSecondary + 'E0',
                border: `1px solid ${theme.colors.border}`,
                boxShadow: `0 4px 20px ${theme.colors.shadowLight}`
            }}
        >
            {/* Header */}
            <div className="flex items-start justify-between mb-4">
                <div className="flex items-start space-x-3 flex-1 min-w-0">
                    {getStatusIcon()}
                    <div className="flex-1 min-w-0">
                        <h3 className="text-lg font-semibold truncate" style={{ color: theme.colors.text }}>
                            {transfer.filename}
                        </h3>
                        <p className="text-sm mt-1" style={{ color: theme.colors.textSecondary }}>
                            {transfer.direction === 'Incoming' ? 'From' : 'To'} {transfer.source_device_name}
                        </p>
                    </div>
                </div>

                {/* Action Buttons */}
                {isActive && (
                    <div className="flex items-center space-x-2">
                        {transfer.status !== 'Connecting' && (
                            <button
                                onClick={handlePauseResume}
                                onMouseEnter={() => setHoveredButton('pause')}
                                onMouseLeave={() => setHoveredButton(null)}
                                className="p-2 rounded-lg transition-all"
                                style={{
                                    backgroundColor: hoveredButton === 'pause' ? theme.colors.backgroundTertiary : 'transparent',
                                    color: hoveredButton === 'pause' ? theme.colors.text : theme.colors.textSecondary
                                }}
                            >
                                {transfer.is_paused || transfer.status === 'Paused' ? (
                                    <Play className="w-4 h-4" />
                                ) : (
                                    <Pause className="w-4 h-4" />
                                )}
                            </button>
                        )}
                        <button
                            onClick={handleCancel}
                            onMouseEnter={() => setHoveredButton('cancel')}
                            onMouseLeave={() => setHoveredButton(null)}
                            className="p-2 rounded-lg transition-all"
                            style={{
                                backgroundColor: hoveredButton === 'cancel' ? theme.colors.backgroundTertiary : 'transparent',
                                color: hoveredButton === 'cancel' ? theme.colors.error : theme.colors.textSecondary
                            }}
                        >
                            <X className="w-4 h-4" />
                        </button>
                    </div>
                )}
            </div>

            {/* Progress Section */}
            {isActive && (
                <div className="space-y-3">
                    {/* Progress Bar */}
                    <div className="space-y-2">
                        <div className="flex justify-between text-sm">
                            <span style={{ color: getStatusColor() }}>
                                {transfer.status === 'Connecting' ? 'Connecting...' : `${progressPercent.toFixed(1)}%`}
                            </span>
                            {transfer.status === 'InProgress' && (
                                <span style={{ color: theme.colors.textSecondary }}>
                                    {formatSpeed(transfer.speed_bps)} • ETA: {formatETA(transfer.eta_seconds)}
                                </span>
                            )}
                        </div>
                        <div
                            className="w-full rounded-full h-2 overflow-hidden"
                            style={{ backgroundColor: theme.colors.backgroundTertiary }}
                        >
                            <motion.div
                                className="h-full rounded-full"
                                style={{ backgroundColor: getStatusColor() }}
                                initial={{ width: 0 }}
                                animate={{ width: `${progressPercent}%` }}
                                transition={{ duration: 0.5, ease: 'easeOut' }}
                            />
                        </div>
                        <div className="flex justify-between text-xs" style={{ color: theme.colors.textSecondary }}>
                            <span>{formatBytes(transfer.transferred_bytes)}</span>
                            <span>{formatBytes(transfer.file_size)}</span>
                        </div>
                    </div>
                </div>
            )}

            {/* Completion Status */}
            {!isActive && (
                <div className="mt-3">
                    <div className="flex items-center justify-between">
                        <span className="text-sm" style={{ color: getStatusColor() }}>
                            {transfer.status === 'Completed' && 'Transfer completed successfully'}
                            {transfer.status === 'Failed' && `Failed: ${transfer.error || 'Unknown error'}`}
                            {transfer.status === 'Cancelled' && 'Transfer cancelled'}
                        </span>
                        <span className="text-sm" style={{ color: theme.colors.textSecondary }}>
                            {formatBytes(transfer.file_size)}
                        </span>
                    </div>
                </div>
            )}
        </motion.div>
    );
};

export default TransferProgressItem;