import React, { useState, useEffect } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import {
    Download,
    Upload,
    CheckCircle,
    XCircle,
    Pause,
    Play,
    X,
    FileText,
    Minimize2
} from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { useTheme } from '../context/ThemeContext';

interface Transfer {
    id: string;
    fileName: string;
    fileSize: number;
    progress: number;
    speed: number;
    direction: 'upload' | 'download';
    status: 'active' | 'paused' | 'completed' | 'error';
    peerId: string;
    peerName: string;
    startTime: number;
    error?: string;
}

interface TransferProgressProps {
    isVisible: boolean;
    onToggle: () => void;
}

const TransferProgress: React.FC<TransferProgressProps> = ({ isVisible, onToggle }) => {
    const { theme } = useTheme();
    const [transfers, setTransfers] = useState<Transfer[]>([]);
    const [isMinimized, setIsMinimized] = useState(false);
    const [hoveredButton, setHoveredButton] = useState<string | null>(null);

    useEffect(() => {
        if (isVisible) {
            loadActiveTransfers();
            const interval = setInterval(loadActiveTransfers, 1000);
            return () => clearInterval(interval);
        }
    }, [isVisible]);

    const loadActiveTransfers = async () => {
        try {
            const activeTransfers = await invoke<Transfer[]>('get_active_transfers');
            setTransfers(activeTransfers);
        } catch (error) {
            console.error('Failed to load transfers:', error);
        }
    };

    const handlePauseResume = async (transferId: string) => {
        try {
            await invoke('toggle_transfer_pause', { transferId });
            loadActiveTransfers();
        } catch (error) {
            console.error('Failed to toggle transfer:', error);
        }
    };

    const handleCancel = async (transferId: string) => {
        try {
            await invoke('cancel_transfer', { transferId });
            loadActiveTransfers();
        } catch (error) {
            console.error('Failed to cancel transfer:', error);
        }
    };

    const formatBytes = (bytes: number) => {
        if (bytes === 0) return '0 B';
        const k = 1024;
        const sizes = ['B', 'KB', 'MB', 'GB'];
        const i = Math.floor(Math.log(bytes) / Math.log(k));
        return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
    };

    const formatSpeed = (bytesPerSecond: number) => {
        return formatBytes(bytesPerSecond) + '/s';
    };

    const formatTimeRemaining = (transfer: Transfer) => {
        if (transfer.speed === 0) return '∞';
        const remaining = (transfer.fileSize * (1 - transfer.progress / 100)) / transfer.speed;
        if (remaining < 60) return `${Math.round(remaining)}s`;
        if (remaining < 3600) return `${Math.round(remaining / 60)}m`;
        return `${Math.round(remaining / 3600)}h`;
    };

    const getStatusColor = (status: string) => {
        switch (status) {
            case 'active': return theme.colors.accent2;
            case 'paused': return theme.colors.warning;
            case 'completed': return theme.colors.success;
            case 'error': return theme.colors.error;
            default: return theme.colors.textSecondary;
        }
    };

    const activeTransfers = transfers.filter(t => t.status === 'active' || t.status === 'paused');
    const completedTransfers = transfers.filter(t => t.status === 'completed' || t.status === 'error');

    if (!isVisible) return null;

    return (
        <AnimatePresence>
            <motion.div
                initial={{ opacity: 0, y: 50 }}
                animate={{ opacity: 1, y: 0 }}
                exit={{ opacity: 0, y: 50 }}
                className="fixed bottom-4 right-4 z-50 backdrop-blur-xl rounded-lg border shadow-2xl"
                style={{
                    backgroundColor: theme.colors.backgroundSecondary + 'F0',
                    borderColor: theme.colors.border,
                    boxShadow: `0 25px 50px -12px ${theme.colors.shadowStrong}`,
                    width: isMinimized ? '300px' : '400px'
                }}
            >
                {/* Header */}
                <div className="flex items-center justify-between p-3 border-b" style={{ borderColor: theme.colors.border }}>
                    <div className="flex items-center space-x-2">
                        <Download className="w-4 h-4" style={{ color: theme.colors.accent2 }} />
                        <span className="font-medium text-sm" style={{ color: theme.colors.text }}>Transfers</span>
                        {activeTransfers.length > 0 && (
                            <span
                                className="text-xs px-2 py-1 rounded-full"
                                style={{
                                    backgroundColor: theme.colors.accent2,
                                    color: theme.colors.text
                                }}
                            >
                                {activeTransfers.length}
                            </span>
                        )}
                    </div>
                    <div className="flex items-center space-x-1">
                        <button
                            onClick={() => setIsMinimized(!isMinimized)}
                            onMouseEnter={() => setHoveredButton('minimize')}
                            onMouseLeave={() => setHoveredButton(null)}
                            className="p-1 transition-colors"
                            style={{
                                color: hoveredButton === 'minimize' ? theme.colors.text : theme.colors.textSecondary
                            }}
                        >
                            <Minimize2 className="w-4 h-4" />
                        </button>
                        <button
                            onClick={onToggle}
                            onMouseEnter={() => setHoveredButton('close')}
                            onMouseLeave={() => setHoveredButton(null)}
                            className="p-1 transition-colors"
                            style={{
                                color: hoveredButton === 'close' ? theme.colors.text : theme.colors.textSecondary
                            }}
                        >
                            <X className="w-4 h-4" />
                        </button>
                    </div>
                </div>

                {!isMinimized && (
                    <motion.div
                        initial={{ height: 0 }}
                        animate={{ height: 'auto' }}
                        exit={{ height: 0 }}
                        className="max-h-96 overflow-y-auto"
                    >
                        {/* Active Transfers */}
                        {activeTransfers.length > 0 && (
                            <div className="p-3 space-y-3">
                                <h4 className="text-xs font-medium uppercase tracking-wide" style={{ color: theme.colors.textSecondary }}>
                                    Active ({activeTransfers.length})
                                </h4>
                                {activeTransfers.map((transfer) => (
                                    <motion.div
                                        key={transfer.id}
                                        layout
                                        className="rounded-lg p-3 space-y-2"
                                        style={{ backgroundColor: theme.colors.backgroundTertiary + '80' }}
                                    >
                                        <div className="flex items-center justify-between">
                                            <div className="flex items-center space-x-2 min-w-0 flex-1">
                                                {transfer.direction === 'upload' ? (
                                                    <Upload className="w-4 h-4 flex-shrink-0" style={{ color: theme.colors.success }} />
                                                ) : (
                                                    <Download className="w-4 h-4 flex-shrink-0" style={{ color: theme.colors.accent2 }} />
                                                )}
                                                <div className="min-w-0 flex-1">
                                                    <p className="text-sm truncate" style={{ color: theme.colors.text }}>{transfer.fileName}</p>
                                                    <p className="text-xs" style={{ color: theme.colors.textSecondary }}>
                                                        {transfer.direction === 'upload' ? 'to' : 'from'} {transfer.peerName}
                                                    </p>
                                                </div>
                                            </div>
                                            <div className="flex items-center space-x-1">
                                                <button
                                                    onClick={() => handlePauseResume(transfer.id)}
                                                    onMouseEnter={() => setHoveredButton(`pause-${transfer.id}`)}
                                                    onMouseLeave={() => setHoveredButton(null)}
                                                    className="p-1 transition-colors"
                                                    style={{
                                                        color: hoveredButton === `pause-${transfer.id}` ? theme.colors.text : theme.colors.textSecondary
                                                    }}
                                                >
                                                    {transfer.status === 'paused' ? (
                                                        <Play className="w-3 h-3" />
                                                    ) : (
                                                        <Pause className="w-3 h-3" />
                                                    )}
                                                </button>
                                                <button
                                                    onClick={() => handleCancel(transfer.id)}
                                                    onMouseEnter={() => setHoveredButton(`cancel-${transfer.id}`)}
                                                    onMouseLeave={() => setHoveredButton(null)}
                                                    className="p-1 transition-colors"
                                                    style={{
                                                        color: hoveredButton === `cancel-${transfer.id}` ? theme.colors.error : theme.colors.textSecondary
                                                    }}
                                                >
                                                    <X className="w-3 h-3" />
                                                </button>
                                            </div>
                                        </div>

                                        {/* Progress Bar */}
                                        <div className="space-y-1">
                                            <div className="flex justify-between text-xs">
                                                <span style={{ color: getStatusColor(transfer.status) }}>
                                                    {transfer.progress.toFixed(1)}%
                                                </span>
                                                <span style={{ color: theme.colors.textSecondary }}>
                                                    {formatSpeed(transfer.speed)} • {formatTimeRemaining(transfer)}
                                                </span>
                                            </div>
                                            <div
                                                className="w-full rounded-full h-1.5"
                                                style={{ backgroundColor: theme.colors.backgroundTertiary }}
                                            >
                                                <motion.div
                                                    className="h-1.5 rounded-full"
                                                    style={{ backgroundColor: theme.colors.accent2 }}
                                                    initial={{ width: 0 }}
                                                    animate={{ width: `${transfer.progress}%` }}
                                                    transition={{ duration: 0.3 }}
                                                />
                                            </div>
                                            <div className="flex justify-between text-xs" style={{ color: theme.colors.textSecondary }}>
                                                <span>{formatBytes(transfer.fileSize * transfer.progress / 100)}</span>
                                                <span>{formatBytes(transfer.fileSize)}</span>
                                            </div>
                                        </div>
                                    </motion.div>
                                ))}
                            </div>
                        )}

                        {/* Recent Transfers */}
                        {completedTransfers.length > 0 && (
                            <div className="p-3 border-t space-y-2" style={{ borderColor: theme.colors.border }}>
                                <h4 className="text-xs font-medium uppercase tracking-wide" style={{ color: theme.colors.textSecondary }}>
                                    Recent
                                </h4>
                                {completedTransfers.slice(0, 3).map((transfer) => (
                                    <motion.div
                                        key={transfer.id}
                                        layout
                                        className="flex items-center space-x-3 p-2 rounded"
                                        style={{ backgroundColor: theme.colors.backgroundTertiary + '50' }}
                                    >
                                        <div className="flex-shrink-0">
                                            {transfer.status === 'completed' ? (
                                                <CheckCircle className="w-4 h-4" style={{ color: theme.colors.success }} />
                                            ) : (
                                                <XCircle className="w-4 h-4" style={{ color: theme.colors.error }} />
                                            )}
                                        </div>
                                        <div className="min-w-0 flex-1">
                                            <p className="text-sm truncate" style={{ color: theme.colors.text }}>{transfer.fileName}</p>
                                            <p className="text-xs" style={{ color: theme.colors.textSecondary }}>
                                                {transfer.status === 'completed' ? 'Completed' : 'Failed'}
                                            </p>
                                        </div>
                                    </motion.div>
                                ))}
                            </div>
                        )}

                        {transfers.length === 0 && (
                            <div className="p-6 text-center">
                                <FileText className="w-8 h-8 mx-auto mb-2" style={{ color: theme.colors.textSecondary }} />
                                <p className="text-sm" style={{ color: theme.colors.textSecondary }}>No transfers</p>
                            </div>
                        )}
                    </motion.div>
                )}
            </motion.div>
        </AnimatePresence>
    );
};

export default TransferProgress;