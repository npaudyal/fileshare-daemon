import React, { useState, useEffect, useRef } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { useTheme } from '../../context/ThemeContext';
import TransferItem from './TransferItem';
import { TransferInfo } from './types';
import {
    Download,
    Minimize2,
    X,
    Settings,
    Gauge,
    FileText,
    Check,
    AlertCircle
} from 'lucide-react';

interface TransferProgressWindowProps {
    className?: string;
}

const TransferProgressWindow: React.FC<TransferProgressWindowProps> = ({ className }) => {
    const { theme } = useTheme();
    const [transfers, setTransfers] = useState<TransferInfo[]>([]);
    const [isMinimized, setIsMinimized] = useState(false);
    const [showSettings, setShowSettings] = useState(false);
    const [bandwidthLimit, setBandwidthLimit] = useState<number | null>(null);
    const [globalStats, setGlobalStats] = useState({
        totalSpeed: 0,
        activeCount: 0,
        completedCount: 0,
        failedCount: 0
    });

    // Auto-refresh and event listener cleanup
    const refreshInterval = useRef<number | null>(null);
    const eventUnsubscribers = useRef<(() => void)[]>([]);

    // Load transfers and setup event listeners
    useEffect(() => {
        loadTransfers();
        setupEventListeners();
        startRefreshInterval();

        return () => {
            if (refreshInterval.current) {
                clearInterval(refreshInterval.current);
            }
            eventUnsubscribers.current.forEach(unsub => unsub());
        };
    }, []);

    const loadTransfers = async () => {
        try {
            const allTransfers = await invoke<TransferInfo[]>('get_all_transfers');
            setTransfers(allTransfers);
            updateGlobalStats(allTransfers);
        } catch (error) {
            console.error('Failed to load transfers:', error);
        }
    };

    const setupEventListeners = async () => {
        try {
            // Listen for transfer progress updates
            const progressUnsub = await listen('transfer:progress', (event: any) => {
                const transferId = event.payload.id;
                const update = event.payload;
                setTransfers(prev =>
                    prev.map(t => t.id === transferId ? { ...t, ...update } : t)
                );
            });

            // Listen for transfer state changes
            const stateUnsub = await listen('transfer:state', (event: any) => {
                const transferId = event.payload.id;
                const newState = event.payload.state;
                setTransfers(prev =>
                    prev.map(t => t.id === transferId ? { ...t, state: newState } : t)
                );
            });

            // Listen for transfer completion
            const completeUnsub = await listen('transfer:completed', (event: any) => {
                const transferId = event.payload;
                setTransfers(prev =>
                    prev.map(t => t.id === transferId
                        ? { ...t, state: 'Completed', progress: 100 }
                        : t
                    )
                );
                showCompletionNotification();
            });

            // Listen for transfer failures
            const failUnsub = await listen('transfer:failed', (event: any) => {
                const { id: transferId, error } = event.payload;
                setTransfers(prev =>
                    prev.map(t => t.id === transferId
                        ? { ...t, state: 'Failed', error }
                        : t
                    )
                );
            });

            eventUnsubscribers.current = [progressUnsub, stateUnsub, completeUnsub, failUnsub];
        } catch (error) {
            console.error('Failed to setup event listeners:', error);
        }
    };

    const startRefreshInterval = () => {
        refreshInterval.current = setInterval(() => {
            loadTransfers();
        }, 2000); // Refresh every 2 seconds as fallback
    };

    const updateGlobalStats = (transfers: TransferInfo[]) => {
        const activeTransfers = transfers.filter(t => t.state === 'Active');
        const totalSpeed = activeTransfers.reduce((sum, t) => sum + t.speed, 0);
        const completedCount = transfers.filter(t => t.state === 'Completed').length;
        const failedCount = transfers.filter(t => t.state === 'Failed').length;

        setGlobalStats({
            totalSpeed,
            activeCount: activeTransfers.length,
            completedCount,
            failedCount
        });
    };

    const showCompletionNotification = () => {
        // Play system notification sound
        if ('Notification' in window && Notification.permission === 'granted') {
            new Notification('Transfer Complete', {
                body: 'File transfer completed successfully',
                icon: '/vite.svg'
            });
        }
    };

    const handlePauseResume = async (transferId: string) => {
        try {
            await invoke('toggle_transfer_pause', { transferId });
        } catch (error) {
            console.error('Failed to toggle transfer:', error);
        }
    };

    const handleCancel = async (transferId: string) => {
        try {
            await invoke('cancel_transfer', { transferId });
        } catch (error) {
            console.error('Failed to cancel transfer:', error);
        }
    };

    const handleSetPriority = async (transferId: string, priority: number) => {
        try {
            await invoke('set_transfer_priority', { transferId, priority });
        } catch (error) {
            console.error('Failed to set priority:', error);
        }
    };

    const handleBandwidthLimitChange = async (limit: number | null) => {
        try {
            await invoke('set_bandwidth_limit', { limitBytesPerSec: limit });
            setBandwidthLimit(limit);
        } catch (error) {
            console.error('Failed to set bandwidth limit:', error);
        }
    };

    const activeTransfers = transfers.filter(t =>
        t.state === 'Active' || t.state === 'Paused' || t.state === 'Connecting'
    );
    const completedTransfers = transfers.filter(t =>
        t.state === 'Completed' || t.state === 'Failed'
    );

    const formatSpeed = (bytesPerSecond: number) => {
        if (bytesPerSecond === 0) return '0 B/s';
        const units = ['B/s', 'KB/s', 'MB/s', 'GB/s'];
        const i = Math.floor(Math.log(bytesPerSecond) / Math.log(1024));
        return `${(bytesPerSecond / Math.pow(1024, i)).toFixed(1)} ${units[i]}`;
    };

    return (
        <div
            className={`h-screen w-full flex flex-col ${className}`}
            style={{
                backgroundColor: theme.colors.background,
                color: theme.colors.text
            }}
        >
            {/* Header */}
            <div
                className="flex items-center justify-between p-3 border-b"
                style={{ borderColor: theme.colors.border }}
            >
                <div className="flex items-center space-x-3">
                    <div className="flex items-center space-x-2">
                        <Download className="w-4 h-4" style={{ color: theme.colors.accent2 }} />
                        <span className="font-medium text-sm">Transfers</span>
                    </div>

                    {/* Global stats */}
                    <div className="flex items-center space-x-4 text-xs" style={{ color: theme.colors.textSecondary }}>
                        {globalStats.activeCount > 0 && (
                            <div className="flex items-center space-x-1">
                                <motion.div
                                    className="w-2 h-2 rounded-full"
                                    style={{ backgroundColor: theme.colors.success }}
                                    animate={{ scale: [1, 1.2, 1] }}
                                    transition={{ repeat: Infinity, duration: 1.5 }}
                                />
                                <span>{globalStats.activeCount} active</span>
                            </div>
                        )}

                        {globalStats.totalSpeed > 0 && (
                            <div className="flex items-center space-x-1">
                                <Gauge className="w-3 h-3" />
                                <span>{formatSpeed(globalStats.totalSpeed)}</span>
                            </div>
                        )}
                    </div>
                </div>

                <div className="flex items-center space-x-1">
                    <button
                        onClick={() => setShowSettings(!showSettings)}
                        className="p-1 rounded transition-colors hover:bg-opacity-10"
                        style={{
                            color: theme.colors.textSecondary,
                            backgroundColor: showSettings ? theme.colors.accent2 + '20' : 'transparent'
                        }}
                    >
                        <Settings className="w-4 h-4" />
                    </button>

                    <button
                        onClick={() => setIsMinimized(!isMinimized)}
                        className="p-1 rounded transition-colors hover:bg-opacity-10"
                        style={{ color: theme.colors.textSecondary }}
                    >
                        <Minimize2 className="w-4 h-4" />
                    </button>

                    <button
                        onClick={() => window.close?.()}
                        className="p-1 rounded transition-colors hover:bg-opacity-10"
                        style={{ color: theme.colors.textSecondary }}
                    >
                        <X className="w-4 h-4" />
                    </button>
                </div>
            </div>

            {/* Settings Panel */}
            <AnimatePresence>
                {showSettings && !isMinimized && (
                    <motion.div
                        initial={{ height: 0 }}
                        animate={{ height: 'auto' }}
                        exit={{ height: 0 }}
                        className="border-b overflow-hidden"
                        style={{ borderColor: theme.colors.border }}
                    >
                        <div className="p-3 space-y-3">
                            <div>
                                <label className="text-xs font-medium" style={{ color: theme.colors.textSecondary }}>
                                    Bandwidth Limit
                                </label>
                                <div className="flex items-center space-x-2 mt-1">
                                    <input
                                        type="number"
                                        placeholder="MB/s (empty = unlimited)"
                                        className="flex-1 px-2 py-1 text-xs rounded border"
                                        style={{
                                            backgroundColor: theme.colors.backgroundSecondary,
                                            borderColor: theme.colors.border,
                                            color: theme.colors.text
                                        }}
                                        value={bandwidthLimit ? bandwidthLimit / (1024 * 1024) : ''}
                                        onChange={(e) => {
                                            const value = e.target.value;
                                            const limit = value ? parseFloat(value) * 1024 * 1024 : null;
                                            handleBandwidthLimitChange(limit);
                                        }}
                                    />
                                </div>
                            </div>
                        </div>
                    </motion.div>
                )}
            </AnimatePresence>

            {/* Content */}
            <AnimatePresence>
                {!isMinimized && (
                    <motion.div
                        initial={{ height: 0 }}
                        animate={{ height: 'auto' }}
                        exit={{ height: 0 }}
                        className="flex-1 overflow-hidden flex flex-col"
                    >
                        <div className="flex-1 overflow-y-auto">
                            {/* Active Transfers */}
                            {activeTransfers.length > 0 && (
                                <div className="p-3 space-y-2">
                                    <h4
                                        className="text-xs font-medium uppercase tracking-wide"
                                        style={{ color: theme.colors.textSecondary }}
                                    >
                                        Active ({activeTransfers.length})
                                    </h4>

                                    <div className="space-y-2">
                                        {activeTransfers.map((transfer) => (
                                            <TransferItem
                                                key={transfer.id}
                                                transfer={transfer}
                                                onPauseResume={handlePauseResume}
                                                onCancel={handleCancel}
                                                onSetPriority={handleSetPriority}
                                            />
                                        ))}
                                    </div>
                                </div>
                            )}

                            {/* Completed Transfers */}
                            {completedTransfers.length > 0 && (
                                <div className="p-3 border-t space-y-2" style={{ borderColor: theme.colors.border }}>
                                    <h4
                                        className="text-xs font-medium uppercase tracking-wide"
                                        style={{ color: theme.colors.textSecondary }}
                                    >
                                        Recent ({completedTransfers.length})
                                    </h4>

                                    <div className="space-y-1">
                                        {completedTransfers.slice(0, 5).map((transfer) => (
                                            <motion.div
                                                key={transfer.id}
                                                layout
                                                className="flex items-center space-x-3 p-2 rounded"
                                                style={{ backgroundColor: theme.colors.backgroundTertiary + '50' }}
                                            >
                                                <div className="flex-shrink-0">
                                                    {transfer.state === 'Completed' ? (
                                                        <Check className="w-4 h-4" style={{ color: theme.colors.success }} />
                                                    ) : (
                                                        <AlertCircle className="w-4 h-4" style={{ color: theme.colors.error }} />
                                                    )}
                                                </div>

                                                <div className="min-w-0 flex-1">
                                                    <p className="text-sm truncate" style={{ color: theme.colors.text }}>
                                                        {transfer.fileName}
                                                    </p>
                                                    <p className="text-xs" style={{ color: theme.colors.textSecondary }}>
                                                        {transfer.state === 'Completed' ? 'Completed' : `Failed: ${transfer.error || 'Unknown error'}`}
                                                    </p>
                                                </div>
                                            </motion.div>
                                        ))}
                                    </div>
                                </div>
                            )}

                            {/* Empty State */}
                            {transfers.length === 0 && (
                                <div className="flex-1 flex items-center justify-center p-8">
                                    <div className="text-center">
                                        <FileText
                                            className="w-12 h-12 mx-auto mb-3"
                                            style={{ color: theme.colors.textSecondary }}
                                        />
                                        <p className="text-sm" style={{ color: theme.colors.textSecondary }}>
                                            No transfers yet
                                        </p>
                                        <p className="text-xs mt-1" style={{ color: theme.colors.textSecondary }}>
                                            Start a file transfer to see progress here
                                        </p>
                                    </div>
                                </div>
                            )}
                        </div>
                    </motion.div>
                )}
            </AnimatePresence>
        </div>
    );
};

export default TransferProgressWindow;