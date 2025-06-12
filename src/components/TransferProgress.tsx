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
    const [transfers, setTransfers] = useState<Transfer[]>([]);
    const [isMinimized, setIsMinimized] = useState(false);

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
            case 'active': return 'text-blue-400';
            case 'paused': return 'text-yellow-400';
            case 'completed': return 'text-green-400';
            case 'error': return 'text-red-400';
            default: return 'text-gray-400';
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
                className="fixed bottom-4 right-4 z-50 bg-slate-800/95 backdrop-blur-xl rounded-lg border border-white/10 shadow-2xl"
                style={{ width: isMinimized ? '300px' : '400px' }}
            >
                {/* Header */}
                <div className="flex items-center justify-between p-3 border-b border-white/10">
                    <div className="flex items-center space-x-2">
                        <Download className="w-4 h-4 text-blue-400" />
                        <span className="text-white font-medium text-sm">Transfers</span>
                        {activeTransfers.length > 0 && (
                            <span className="bg-blue-500 text-white text-xs px-2 py-1 rounded-full">
                                {activeTransfers.length}
                            </span>
                        )}
                    </div>
                    <div className="flex items-center space-x-1">
                        <button
                            onClick={() => setIsMinimized(!isMinimized)}
                            className="p-1 text-gray-400 hover:text-white transition-colors"
                        >
                            <Minimize2 className="w-4 h-4" />
                        </button>
                        <button
                            onClick={onToggle}
                            className="p-1 text-gray-400 hover:text-white transition-colors"
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
                                <h4 className="text-xs font-medium text-gray-400 uppercase tracking-wide">
                                    Active ({activeTransfers.length})
                                </h4>
                                {activeTransfers.map((transfer) => (
                                    <motion.div
                                        key={transfer.id}
                                        layout
                                        className="bg-white/5 rounded-lg p-3 space-y-2"
                                    >
                                        <div className="flex items-center justify-between">
                                            <div className="flex items-center space-x-2 min-w-0 flex-1">
                                                {transfer.direction === 'upload' ? (
                                                    <Upload className="w-4 h-4 text-green-400 flex-shrink-0" />
                                                ) : (
                                                    <Download className="w-4 h-4 text-blue-400 flex-shrink-0" />
                                                )}
                                                <div className="min-w-0 flex-1">
                                                    <p className="text-white text-sm truncate">{transfer.fileName}</p>
                                                    <p className="text-gray-400 text-xs">
                                                        {transfer.direction === 'upload' ? 'to' : 'from'} {transfer.peerName}
                                                    </p>
                                                </div>
                                            </div>
                                            <div className="flex items-center space-x-1">
                                                <button
                                                    onClick={() => handlePauseResume(transfer.id)}
                                                    className="p-1 text-gray-400 hover:text-white transition-colors"
                                                >
                                                    {transfer.status === 'paused' ? (
                                                        <Play className="w-3 h-3" />
                                                    ) : (
                                                        <Pause className="w-3 h-3" />
                                                    )}
                                                </button>
                                                <button
                                                    onClick={() => handleCancel(transfer.id)}
                                                    className="p-1 text-gray-400 hover:text-red-400 transition-colors"
                                                >
                                                    <X className="w-3 h-3" />
                                                </button>
                                            </div>
                                        </div>

                                        {/* Progress Bar */}
                                        <div className="space-y-1">
                                            <div className="flex justify-between text-xs">
                                                <span className={getStatusColor(transfer.status)}>
                                                    {transfer.progress.toFixed(1)}%
                                                </span>
                                                <span className="text-gray-400">
                                                    {formatSpeed(transfer.speed)} • {formatTimeRemaining(transfer)}
                                                </span>
                                            </div>
                                            <div className="w-full bg-gray-700 rounded-full h-1.5">
                                                <motion.div
                                                    className="bg-blue-500 h-1.5 rounded-full"
                                                    initial={{ width: 0 }}
                                                    animate={{ width: `${transfer.progress}%` }}
                                                    transition={{ duration: 0.3 }}
                                                />
                                            </div>
                                            <div className="flex justify-between text-xs text-gray-400">
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
                            <div className="p-3 border-t border-white/10 space-y-2">
                                <h4 className="text-xs font-medium text-gray-400 uppercase tracking-wide">
                                    Recent
                                </h4>
                                {completedTransfers.slice(0, 3).map((transfer) => (
                                    <motion.div
                                        key={transfer.id}
                                        layout
                                        className="flex items-center space-x-3 p-2 bg-white/5 rounded"
                                    >
                                        <div className="flex-shrink-0">
                                            {transfer.status === 'completed' ? (
                                                <CheckCircle className="w-4 h-4 text-green-400" />
                                            ) : (
                                                <XCircle className="w-4 h-4 text-red-400" />
                                            )}
                                        </div>
                                        <div className="min-w-0 flex-1">
                                            <p className="text-white text-sm truncate">{transfer.fileName}</p>
                                            <p className="text-gray-400 text-xs">
                                                {transfer.status === 'completed' ? 'Completed' : 'Failed'}
                                            </p>
                                        </div>
                                    </motion.div>
                                ))}
                            </div>
                        )}

                        {transfers.length === 0 && (
                            <div className="p-6 text-center">
                                <FileText className="w-8 h-8 text-gray-400 mx-auto mb-2" />
                                <p className="text-gray-400 text-sm">No transfers</p>
                            </div>
                        )}
                    </motion.div>
                )}
            </motion.div>
        </AnimatePresence>
    );
};

export default TransferProgress;