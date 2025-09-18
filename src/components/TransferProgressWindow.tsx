import React, { useState, useEffect, useCallback } from 'react';
import { motion } from 'framer-motion';
import {
    Download,
    Upload,
    CheckCircle,
    Pause,
    Play,
    X,
    FileText,
    AlertCircle,
    Clock
} from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { useTheme } from '../context/ThemeContext';

interface Transfer {
    id: string;
    file_name: string;
    file_path: string;
    file_size: number;
    transferred_bytes: number;
    progress: number;
    speed_bps: number;
    direction: 'Upload' | 'Download';
    status: 'Preparing' | 'Active' | 'Paused' | 'Completed' | { Error: string } | 'Cancelled';
    peer_id: string;
    peer_name: string;
    start_time: number;
    end_time?: number;
    time_remaining?: number;
    error?: string;
    is_paused: boolean;
}

const TransferProgressWindow: React.FC = () => {
    const { theme } = useTheme();
    const [transfer, setTransfer] = useState<Transfer | null>(null);
    const [isLoading, setIsLoading] = useState(true);
    const [autoCloseTimer, setAutoCloseTimer] = useState<number | null>(null);
    const [shouldAutoClose, setShouldAutoClose] = useState(true);

    // Get transfer ID from window label
    const getTransferIdFromWindow = useCallback(async () => {
        try {
            const currentWindow = getCurrentWindow();
            const label = currentWindow.label;
            const transferId = label.replace('transfer-', '');
            return transferId;
        } catch (error) {
            console.error('Failed to get transfer ID from window:', error);
            return null;
        }
    }, []);

    // Load transfer data
    const loadTransfer = useCallback(async () => {
        try {
            const transferId = await getTransferIdFromWindow();
            if (!transferId) return;

            const transferData = await invoke<Transfer | null>('get_transfer_status', { transferId });
            if (transferData) {
                setTransfer(transferData);
            }
        } catch (error) {
            console.error('Failed to load transfer:', error);
        } finally {
            setIsLoading(false);
        }
    }, [getTransferIdFromWindow]);

    // Listen for transfer progress events
    useEffect(() => {
        let unlisten: (() => void) | null = null;

        const setupListener = async () => {
            try {
                unlisten = await listen('transfer-progress', (event: any) => {
                    const progressData = event.payload;
                    if (progressData && transfer && progressData.id === transfer.id) {
                        setTransfer(prevTransfer => ({
                            ...prevTransfer!,
                            ...progressData
                        }));
                    }
                });
            } catch (error) {
                console.error('Failed to setup progress listener:', error);
            }
        };

        setupListener();

        return () => {
            if (unlisten) unlisten();
        };
    }, [transfer?.id]);

    // Initial load and periodic refresh
    useEffect(() => {
        loadTransfer();
        const interval = setInterval(loadTransfer, 1000);
        return () => clearInterval(interval);
    }, [loadTransfer]);

    // Auto-close logic
    useEffect(() => {
        if (transfer && shouldAutoClose) {
            const status = typeof transfer.status === 'string' ? transfer.status : 'Error';

            if (status === 'Completed' && !autoCloseTimer) {
                const timer = window.setTimeout(async () => {
                    try {
                        const currentWindow = getCurrentWindow();
                        await currentWindow.close();
                    } catch (error) {
                        console.error('Failed to close window:', error);
                    }
                }, 3000);
                setAutoCloseTimer(timer);
            }
        }

        return () => {
            if (autoCloseTimer) {
                clearTimeout(autoCloseTimer);
                setAutoCloseTimer(null);
            }
        };
    }, [transfer?.status, shouldAutoClose, autoCloseTimer]);

    const handlePauseResume = async () => {
        if (!transfer) return;

        try {
            await invoke('toggle_transfer_pause', { transferId: transfer.id });
            await loadTransfer();
        } catch (error) {
            console.error('Failed to toggle transfer:', error);
        }
    };

    const handleCancel = async () => {
        if (!transfer) return;

        try {
            await invoke('cancel_transfer', { transferId: transfer.id });
            await loadTransfer();
        } catch (error) {
            console.error('Failed to cancel transfer:', error);
        }
    };

    const handleClose = async () => {
        try {
            const currentWindow = getCurrentWindow();
            await currentWindow.close();
        } catch (error) {
            console.error('Failed to close window:', error);
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

    const formatTimeRemaining = (seconds?: number) => {
        if (!seconds || seconds === 0) return '∞';
        if (seconds < 60) return `${Math.round(seconds)}s`;
        if (seconds < 3600) return `${Math.round(seconds / 60)}m`;
        return `${Math.round(seconds / 3600)}h`;
    };

    const getStatusColor = (status: Transfer['status']) => {
        if (typeof status === 'string') {
            switch (status) {
                case 'Active': return theme.colors.accent2;
                case 'Paused': return theme.colors.warning;
                case 'Completed': return theme.colors.success;
                case 'Cancelled': return theme.colors.textSecondary;
                case 'Preparing': return theme.colors.accent1;
                default: return theme.colors.textSecondary;
            }
        } else {
            return theme.colors.error;
        }
    };

    const getStatusText = (status: Transfer['status']) => {
        if (typeof status === 'string') {
            return status;
        } else {
            return 'Error';
        }
    };

    if (isLoading) {
        return (
            <div
                className="w-full h-full flex items-center justify-center backdrop-blur-xl"
                style={{ backgroundColor: theme.colors.backgroundSecondary + 'F0' }}
            >
                <div className="text-center">
                    <Clock className="w-8 h-8 mx-auto mb-2 animate-pulse" style={{ color: theme.colors.accent2 }} />
                    <p className="text-sm" style={{ color: theme.colors.text }}>Loading transfer...</p>
                </div>
            </div>
        );
    }

    if (!transfer) {
        return (
            <div
                className="w-full h-full flex items-center justify-center backdrop-blur-xl"
                style={{ backgroundColor: theme.colors.backgroundSecondary + 'F0' }}
            >
                <div className="text-center">
                    <AlertCircle className="w-8 h-8 mx-auto mb-2" style={{ color: theme.colors.error }} />
                    <p className="text-sm" style={{ color: theme.colors.text }}>Transfer not found</p>
                    <button
                        onClick={handleClose}
                        className="mt-2 px-3 py-1 rounded text-xs"
                        style={{
                            backgroundColor: theme.colors.accent2,
                            color: theme.colors.text
                        }}
                    >
                        Close
                    </button>
                </div>
            </div>
        );
    }

    const statusText = getStatusText(transfer.status);
    const isCompleted = statusText === 'Completed';
    const isError = typeof transfer.status === 'object';
    const canControl = statusText === 'Active' || statusText === 'Paused';

    return (
        <div
            className="w-full h-full backdrop-blur-xl border shadow-2xl"
            style={{
                backgroundColor: theme.colors.backgroundSecondary + 'F0',
                borderColor: theme.colors.border,
                boxShadow: `0 25px 50px -12px ${theme.colors.shadowStrong}`,
            }}
        >
            {/* Header */}
            <div className="flex items-center justify-between p-3 border-b" style={{ borderColor: theme.colors.border }}>
                <div className="flex items-center space-x-2">
                    {transfer.direction === 'Upload' ? (
                        <Upload className="w-4 h-4" style={{ color: theme.colors.success }} />
                    ) : (
                        <Download className="w-4 h-4" style={{ color: theme.colors.accent2 }} />
                    )}
                    <span className="text-sm font-medium" style={{ color: theme.colors.text }}>
                        {transfer.direction === 'Upload' ? 'Sending to' : 'Receiving from'} {transfer.peer_name}
                    </span>
                </div>
                <button
                    onClick={handleClose}
                    className="p-1 hover:bg-opacity-10 hover:bg-white rounded transition-colors"
                    style={{ color: theme.colors.textSecondary }}
                >
                    <X className="w-4 h-4" />
                </button>
            </div>

            {/* Content */}
            <div className="p-4 space-y-4">
                {/* File Info */}
                <div className="flex items-center space-x-3">
                    <FileText className="w-8 h-8 flex-shrink-0" style={{ color: theme.colors.accent1 }} />
                    <div className="min-w-0 flex-1">
                        <p className="text-sm font-medium truncate" style={{ color: theme.colors.text }}>
                            {transfer.file_name}
                        </p>
                        <p className="text-xs" style={{ color: theme.colors.textSecondary }}>
                            {formatBytes(transfer.file_size)}
                        </p>
                    </div>
                </div>

                {/* Progress */}
                <div className="space-y-2">
                    <div className="flex items-center justify-between text-xs">
                        <span style={{ color: getStatusColor(transfer.status) }}>
                            {statusText} • {transfer.progress.toFixed(1)}%
                        </span>
                        {!isCompleted && !isError && (
                            <span style={{ color: theme.colors.textSecondary }}>
                                {formatSpeed(transfer.speed_bps)} • {formatTimeRemaining(transfer.time_remaining)}
                            </span>
                        )}
                    </div>

                    {/* Progress Bar */}
                    <div
                        className="w-full rounded-full h-2"
                        style={{ backgroundColor: theme.colors.backgroundTertiary }}
                    >
                        <motion.div
                            className="h-2 rounded-full"
                            style={{ backgroundColor: getStatusColor(transfer.status) }}
                            initial={{ width: 0 }}
                            animate={{ width: `${transfer.progress}%` }}
                            transition={{ duration: 0.3 }}
                        />
                    </div>

                    <div className="flex justify-between text-xs" style={{ color: theme.colors.textSecondary }}>
                        <span>{formatBytes(transfer.transferred_bytes)}</span>
                        <span>{formatBytes(transfer.file_size)}</span>
                    </div>
                </div>

                {/* Status Message */}
                {isError && (
                    <div className="p-2 rounded" style={{ backgroundColor: theme.colors.error + '20' }}>
                        <p className="text-xs" style={{ color: theme.colors.error }}>
                            {transfer.error || 'Transfer failed'}
                        </p>
                    </div>
                )}

                {isCompleted && (
                    <div className="flex items-center space-x-2 p-2 rounded" style={{ backgroundColor: theme.colors.success + '20' }}>
                        <CheckCircle className="w-4 h-4" style={{ color: theme.colors.success }} />
                        <span className="text-xs" style={{ color: theme.colors.success }}>
                            Transfer completed successfully
                        </span>
                    </div>
                )}

                {/* Controls */}
                <div className="flex items-center justify-between">
                    <div className="flex items-center space-x-2">
                        {canControl && (
                            <>
                                <button
                                    onClick={handlePauseResume}
                                    className="p-2 rounded hover:bg-opacity-10 hover:bg-white transition-colors"
                                    style={{ color: theme.colors.textSecondary }}
                                >
                                    {transfer.is_paused ? (
                                        <Play className="w-4 h-4" />
                                    ) : (
                                        <Pause className="w-4 h-4" />
                                    )}
                                </button>
                                <button
                                    onClick={handleCancel}
                                    className="p-2 rounded hover:bg-opacity-10 hover:bg-white transition-colors"
                                    style={{ color: theme.colors.error }}
                                >
                                    <X className="w-4 h-4" />
                                </button>
                            </>
                        )}
                    </div>

                    {isCompleted && (
                        <div className="flex items-center space-x-2">
                            <label className="flex items-center space-x-1 text-xs" style={{ color: theme.colors.textSecondary }}>
                                <input
                                    type="checkbox"
                                    checked={!shouldAutoClose}
                                    onChange={(e) => setShouldAutoClose(!e.target.checked)}
                                    className="w-3 h-3"
                                />
                                <span>Keep open</span>
                            </label>
                        </div>
                    )}
                </div>

                {/* Auto-close countdown */}
                {isCompleted && shouldAutoClose && autoCloseTimer && (
                    <div className="text-center">
                        <p className="text-xs" style={{ color: theme.colors.textSecondary }}>
                            Closing in 3 seconds...
                        </p>
                    </div>
                )}
            </div>
        </div>
    );
};

export default TransferProgressWindow;