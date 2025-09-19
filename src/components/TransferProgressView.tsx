import React, { useCallback, useEffect, useState } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import {
    ArrowLeft,
    Download,
    Upload,
    FolderOpen,
    CheckCircle2,
    XCircle,
    Activity,
    Minimize2,
    X
} from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { useTheme } from '../context/ThemeContext';
import { useViewStore } from '../stores/viewStore';
import { useTransferView } from '../hooks/useTransferView';
import TransferProgressItem from './TransferProgressItem';

const TransferProgressView: React.FC = () => {
    const { theme } = useTheme();
    const { cancelAllAndReturn, toggleWindowMode } = useTransferView();
    const {
        transfers,
        getActiveTransfers,
        getCompletedTransfers,
        clearCompletedTransfers,
        setWindowMode
    } = useViewStore();

    const [isMinimized, setIsMinimized] = useState(false);

    const activeTransfers = getActiveTransfers();
    const completedTransfers = getCompletedTransfers();
    const allTransfers = Array.from(transfers.values());

    // Auto-refresh active transfers
    useEffect(() => {
        const interval = setInterval(async () => {
            if (activeTransfers.length > 0) {
                try {
                    await invoke('get_active_transfers');
                } catch (error) {
                    console.error('Failed to refresh transfers:', error);
                }
            }
        }, 1000);

        return () => clearInterval(interval);
    }, [activeTransfers.length]);

    const handleReturnToNormal = useCallback(() => {
        setWindowMode('Normal');
    }, [setWindowMode]);

    const formatOverallProgress = () => {
        if (allTransfers.length === 0) return { percent: 0, text: 'No transfers' };

        let totalBytes = 0;
        let transferredBytes = 0;

        allTransfers.forEach(transfer => {
            totalBytes += transfer.file_size;
            transferredBytes += transfer.transferred_bytes;
        });

        const percent = totalBytes > 0 ? (transferredBytes / totalBytes) * 100 : 0;
        return {
            percent,
            text: `${percent.toFixed(1)}% overall`
        };
    };

    const overallProgress = formatOverallProgress();

    return (
        <motion.div
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            className="w-full h-full flex flex-col backdrop-blur-2xl"
            style={{
                background: `linear-gradient(135deg, ${theme.colors.background}F0 0%, ${theme.colors.backgroundSecondary}F0 100%)`,
                borderRadius: '12px'
            }}
        >
            {/* Header */}
            <div
                className="flex items-center justify-between p-6 border-b"
                style={{ borderColor: theme.colors.border }}
            >
                <div className="flex items-center space-x-4">
                    <button
                        onClick={handleReturnToNormal}
                        className="p-2 rounded-lg transition-all hover:scale-105"
                        style={{
                            backgroundColor: theme.colors.backgroundSecondary,
                            color: theme.colors.text
                        }}
                    >
                        <ArrowLeft className="w-5 h-5" />
                    </button>

                    <div>
                        <h1 className="text-2xl font-bold flex items-center space-x-2" style={{ color: theme.colors.text }}>
                            <Activity className="w-6 h-6" style={{ color: theme.colors.accent2 }} />
                            <span>File Transfers</span>
                        </h1>
                        <p className="text-sm mt-1" style={{ color: theme.colors.textSecondary }}>
                            {activeTransfers.length} active, {completedTransfers.length} completed
                        </p>
                    </div>
                </div>

                <div className="flex items-center space-x-3">
                    {/* Overall Progress */}
                    {activeTransfers.length > 0 && (
                        <div className="flex items-center space-x-3 px-4 py-2 rounded-lg"
                            style={{ backgroundColor: theme.colors.backgroundTertiary }}
                        >
                            <div className="text-right">
                                <p className="text-sm font-medium" style={{ color: theme.colors.text }}>
                                    {overallProgress.text}
                                </p>
                                <p className="text-xs" style={{ color: theme.colors.textSecondary }}>
                                    {activeTransfers.length} file{activeTransfers.length !== 1 ? 's' : ''}
                                </p>
                            </div>
                            <div className="w-24 h-2 rounded-full overflow-hidden"
                                style={{ backgroundColor: theme.colors.backgroundSecondary }}
                            >
                                <motion.div
                                    className="h-full rounded-full"
                                    style={{ backgroundColor: theme.colors.accent2 }}
                                    initial={{ width: 0 }}
                                    animate={{ width: `${overallProgress.percent}%` }}
                                />
                            </div>
                        </div>
                    )}

                    {/* Action Buttons */}
                    {activeTransfers.length > 0 && (
                        <button
                            onClick={cancelAllAndReturn}
                            className="px-4 py-2 rounded-lg transition-all hover:scale-105"
                            style={{
                                backgroundColor: theme.colors.error + '20',
                                color: theme.colors.error,
                                border: `1px solid ${theme.colors.error}40`
                            }}
                        >
                            Cancel All
                        </button>
                    )}

                    <button
                        onClick={() => setIsMinimized(!isMinimized)}
                        className="p-2 rounded-lg transition-all"
                        style={{
                            backgroundColor: theme.colors.backgroundSecondary,
                            color: theme.colors.textSecondary
                        }}
                    >
                        <Minimize2 className="w-5 h-5" />
                    </button>
                </div>
            </div>

            {/* Content */}
            <div className="flex-1 overflow-y-auto p-6">
                {allTransfers.length === 0 ? (
                    <motion.div
                        initial={{ opacity: 0, y: 20 }}
                        animate={{ opacity: 1, y: 0 }}
                        className="flex flex-col items-center justify-center h-full"
                    >
                        <FolderOpen className="w-20 h-20 mb-4" style={{ color: theme.colors.textSecondary }} />
                        <h2 className="text-xl font-semibold mb-2" style={{ color: theme.colors.text }}>
                            No Active Transfers
                        </h2>
                        <p className="text-sm" style={{ color: theme.colors.textSecondary }}>
                            Use Cmd+Shift+Y to copy and Cmd+Shift+I to paste files
                        </p>
                        <button
                            onClick={handleReturnToNormal}
                            className="mt-6 px-6 py-2 rounded-lg transition-all hover:scale-105"
                            style={{
                                backgroundColor: theme.colors.accent2,
                                color: theme.colors.text
                            }}
                        >
                            Return to Devices
                        </button>
                    </motion.div>
                ) : (
                    <div className="space-y-6">
                        {/* Active Transfers */}
                        {activeTransfers.length > 0 && (
                            <div>
                                <h2 className="text-lg font-semibold mb-4 flex items-center space-x-2"
                                    style={{ color: theme.colors.text }}
                                >
                                    <Download className="w-5 h-5" style={{ color: theme.colors.accent2 }} />
                                    <span>Active Transfers</span>
                                </h2>
                                <div className="space-y-4">
                                    <AnimatePresence>
                                        {activeTransfers.map(transfer => (
                                            <TransferProgressItem
                                                key={transfer.id}
                                                transfer={transfer}
                                                onUpdate={() => invoke('get_active_transfers')}
                                            />
                                        ))}
                                    </AnimatePresence>
                                </div>
                            </div>
                        )}

                        {/* Completed Transfers */}
                        {completedTransfers.length > 0 && (
                            <div>
                                <div className="flex items-center justify-between mb-4">
                                    <h2 className="text-lg font-semibold flex items-center space-x-2"
                                        style={{ color: theme.colors.text }}
                                    >
                                        <CheckCircle2 className="w-5 h-5" style={{ color: theme.colors.success }} />
                                        <span>Recent Transfers</span>
                                    </h2>
                                    <button
                                        onClick={clearCompletedTransfers}
                                        className="text-sm px-3 py-1 rounded-lg transition-all"
                                        style={{
                                            backgroundColor: theme.colors.backgroundTertiary,
                                            color: theme.colors.textSecondary
                                        }}
                                    >
                                        Clear All
                                    </button>
                                </div>
                                <div className="space-y-4">
                                    <AnimatePresence>
                                        {completedTransfers.slice(0, 5).map(transfer => (
                                            <TransferProgressItem
                                                key={transfer.id}
                                                transfer={transfer}
                                                onUpdate={() => invoke('get_active_transfers')}
                                            />
                                        ))}
                                    </AnimatePresence>
                                </div>
                            </div>
                        )}
                    </div>
                )}
            </div>

            {/* Mini Mode */}
            {isMinimized && (
                <motion.div
                    initial={{ opacity: 0, scale: 0.8 }}
                    animate={{ opacity: 1, scale: 1 }}
                    className="fixed bottom-4 right-4 p-4 rounded-lg shadow-lg backdrop-blur-md"
                    style={{
                        backgroundColor: theme.colors.backgroundSecondary + 'F0',
                        border: `1px solid ${theme.colors.border}`
                    }}
                >
                    <div className="flex items-center space-x-3">
                        <Activity className="w-5 h-5 animate-pulse" style={{ color: theme.colors.accent2 }} />
                        <div>
                            <p className="text-sm font-medium" style={{ color: theme.colors.text }}>
                                {activeTransfers.length} transfer{activeTransfers.length !== 1 ? 's' : ''} in progress
                            </p>
                            <p className="text-xs" style={{ color: theme.colors.textSecondary }}>
                                {overallProgress.text}
                            </p>
                        </div>
                        <button
                            onClick={() => setIsMinimized(false)}
                            className="p-1 rounded transition-all"
                            style={{ color: theme.colors.textSecondary }}
                        >
                            <X className="w-4 h-4" />
                        </button>
                    </div>
                </motion.div>
            )}
        </motion.div>
    );
};

export default TransferProgressView;