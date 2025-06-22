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
    Minimize2,
    Zap,
    Activity,
    Clock,
    Cpu,
    TrendingUp
} from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';

interface Transfer {
    id: string;
    fileName: string;
    fileSize: number;
    progress: number;
    speed: number;
    direction: 'upload' | 'download';
    status: 'active' | 'paused' | 'completed' | 'error' | 'pending';
    peerId: string;
    peerName: string;
    startTime: number;
    error?: string;
    // Enhanced streaming features
    estimatedTimeRemaining?: number;
    averageSpeed?: number;
    currentSpeed?: number;
    isStreaming?: boolean;
    totalChunks?: number;
    completedChunks?: number;
    streamChunkSize?: number;
    memoryUsage?: number;
    compressionRatio?: number;
    peakSpeed?: number;
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
        const sizes = ['B', 'KB', 'MB', 'GB', 'TB', 'PB'];
        const i = Math.floor(Math.log(bytes) / Math.log(k));
        return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
    };

    const formatSpeed = (bytesPerSecond: number) => {
        return formatBytes(bytesPerSecond) + '/s';
    };

    const formatTimeRemaining = (transfer: Transfer) => {
        // Use enhanced ETA if available
        const remaining = transfer.estimatedTimeRemaining ?? 
            (transfer.speed === 0 ? Infinity : (transfer.fileSize * (1 - transfer.progress / 100)) / transfer.speed);
        
        if (!isFinite(remaining)) return '∞';
        if (remaining < 60) return `${Math.round(remaining)}s`;
        if (remaining < 3600) return `${Math.round(remaining / 60)}m`;
        const hours = Math.floor(remaining / 3600);
        const minutes = Math.round((remaining % 3600) / 60);
        return hours > 0 ? `${hours}h ${minutes}m` : `${Math.round(remaining / 60)}m`;
    };

    const getFileSizeCategory = (bytes: number): 'small' | 'medium' | 'large' | 'huge' => {
        if (bytes < 100 * 1024 * 1024) return 'small'; // < 100MB
        if (bytes < 1024 * 1024 * 1024) return 'medium'; // < 1GB
        if (bytes < 10 * 1024 * 1024 * 1024) return 'large'; // < 10GB
        return 'huge'; // >= 10GB
    };

    const getFileSizeBadge = (bytes: number) => {
        const category = getFileSizeCategory(bytes);
        if (category === 'small' || category === 'medium') return null;
        
        const config = {
            large: { label: '1GB+', className: 'bg-yellow-500/20 text-yellow-300' },
            huge: { label: '10GB+', className: 'bg-red-500/20 text-red-300' }
        };
        
        return (
            <span className={`text-xs px-2 py-1 rounded-full ${config[category].className}`}>
                {config[category].label}
            </span>
        );
    };

    const formatCompressionRatio = (ratio?: number) => {
        if (!ratio || ratio === 1) return null;
        const percent = Math.round((1 - ratio) * 100);
        return `${percent}% compressed`;
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

    const pendingTransfers = transfers.filter(t => t.status === 'pending');
    const activeTransfers = transfers.filter(t => t.status === 'active' || t.status === 'paused');
    const completedTransfers = transfers.filter(t => t.status === 'completed' || t.status === 'error');

    // Enhanced transfer card component
    const TransferCard: React.FC<{ transfer: Transfer }> = ({ transfer }) => {
        const isActive = transfer.status === 'active';
        const isStreaming = transfer.isStreaming && isActive;
        const fileSizeBadge = getFileSizeBadge(transfer.fileSize);
        const compressionInfo = formatCompressionRatio(transfer.compressionRatio);
        
        return (
            <motion.div
                layout
                className="bg-white/5 rounded-lg p-3 space-y-3 border border-white/5 hover:border-white/10 transition-colors"
            >
                {/* Header with file info and controls */}
                <div className="flex items-center justify-between">
                    <div className="flex items-center space-x-2 min-w-0 flex-1">
                        {transfer.direction === 'upload' ? (
                            <Upload className="w-4 h-4 text-green-400 flex-shrink-0" />
                        ) : (
                            <Download className="w-4 h-4 text-blue-400 flex-shrink-0" />
                        )}
                        <div className="min-w-0 flex-1">
                            <div className="flex items-center space-x-2">
                                <p className="text-white text-sm truncate font-medium">{transfer.fileName}</p>
                                {fileSizeBadge}
                                {isStreaming && (
                                    <div className="flex items-center space-x-1">
                                        <div className="w-2 h-2 bg-green-400 rounded-full animate-pulse"></div>
                                        <span className="text-green-400 text-xs font-medium">STREAMING</span>
                                    </div>
                                )}
                            </div>
                            <div className="flex items-center space-x-2 text-xs text-gray-400">
                                <span>{transfer.direction === 'upload' ? 'to' : 'from'} {transfer.peerName}</span>
                                {compressionInfo && (
                                    <>
                                        <span>•</span>
                                        <span className="text-purple-400">{compressionInfo}</span>
                                    </>
                                )}
                            </div>
                        </div>
                    </div>
                    <div className="flex items-center space-x-1">
                        {(transfer.status === 'active' || transfer.status === 'paused') && (
                            <>
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
                            </>
                        )}
                    </div>
                </div>

                {/* Progress section */}
                {(transfer.status === 'active' || transfer.status === 'paused') && (
                    <div className="space-y-2">
                        {/* Status and speeds */}
                        <div className="flex justify-between text-xs">
                            <div className="flex items-center space-x-2">
                                <span className={getStatusColor(transfer.status)}>
                                    {transfer.progress.toFixed(1)}%
                                </span>
                                {transfer.totalChunks && transfer.completedChunks !== undefined && (
                                    <span className="text-gray-400">
                                        {transfer.completedChunks}/{transfer.totalChunks} chunks
                                    </span>
                                )}
                            </div>
                            <div className="flex items-center space-x-2 text-gray-400">
                                {transfer.currentSpeed !== undefined && transfer.averageSpeed !== undefined ? (
                                    <>
                                        <span>{formatSpeed(transfer.currentSpeed)}</span>
                                        <span>•</span>
                                        <span>avg {formatSpeed(transfer.averageSpeed)}</span>
                                    </>
                                ) : (
                                    <span>{formatSpeed(transfer.speed)}</span>
                                )}
                                <span>•</span>
                                <span>{formatTimeRemaining(transfer)}</span>
                            </div>
                        </div>

                        {/* Progress bar */}
                        <div className="w-full bg-gray-700 rounded-full h-2 overflow-hidden">
                            <motion.div
                                className={`h-2 rounded-full ${
                                    transfer.status === 'paused' 
                                        ? 'bg-gradient-to-r from-yellow-500 to-orange-500'
                                        : isStreaming
                                            ? 'bg-gradient-to-r from-blue-500 via-purple-500 to-blue-500 bg-[length:200%_100%] animate-gradient'
                                            : 'bg-gradient-to-r from-blue-500 to-purple-500'
                                }`}
                                initial={{ width: 0 }}
                                animate={{ width: `${transfer.progress}%` }}
                                transition={{ duration: 0.3, ease: 'easeOut' }}
                            />
                        </div>

                        {/* File size info */}
                        <div className="flex justify-between text-xs text-gray-400">
                            <span>{formatBytes(transfer.fileSize * transfer.progress / 100)}</span>
                            <span>{formatBytes(transfer.fileSize)}</span>
                        </div>

                        {/* Advanced stats for large files */}
                        {(getFileSizeCategory(transfer.fileSize) === 'large' || getFileSizeCategory(transfer.fileSize) === 'huge') && (
                            <div className="grid grid-cols-2 gap-2 pt-2 border-t border-white/5">
                                {transfer.peakSpeed && (
                                    <div className="flex items-center space-x-1 text-xs">
                                        <TrendingUp className="w-3 h-3 text-green-400" />
                                        <span className="text-gray-400">Peak:</span>
                                        <span className="text-green-400">{formatSpeed(transfer.peakSpeed)}</span>
                                    </div>
                                )}
                                {transfer.memoryUsage && (
                                    <div className="flex items-center space-x-1 text-xs">
                                        <Cpu className="w-3 h-3 text-blue-400" />
                                        <span className="text-gray-400">Memory:</span>
                                        <span className="text-blue-400">{formatBytes(transfer.memoryUsage)}</span>
                                    </div>
                                )}
                                {transfer.streamChunkSize && (
                                    <div className="flex items-center space-x-1 text-xs">
                                        <Activity className="w-3 h-3 text-purple-400" />
                                        <span className="text-gray-400">Chunk:</span>
                                        <span className="text-purple-400">{formatBytes(transfer.streamChunkSize)}</span>
                                    </div>
                                )}
                                <div className="flex items-center space-x-1 text-xs">
                                    <Clock className="w-3 h-3 text-yellow-400" />
                                    <span className="text-gray-400">Elapsed:</span>
                                    <span className="text-yellow-400">{Math.floor(transfer.startTime / 60)}m {transfer.startTime % 60}s</span>
                                </div>
                            </div>
                        )}
                    </div>
                )}
            </motion.div>
        );
    };

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
                        {/* Pending Transfers */}
                        {pendingTransfers.length > 0 && (
                            <div className="p-3 space-y-3">
                                <h4 className="text-xs font-medium text-gray-400 uppercase tracking-wide flex items-center space-x-2">
                                    <Clock className="w-3 h-3" />
                                    <span>Queued ({pendingTransfers.length})</span>
                                </h4>
                                {pendingTransfers.map((transfer) => (
                                    <motion.div
                                        key={transfer.id}
                                        layout
                                        className="bg-white/5 rounded-lg p-3 border-l-2 border-yellow-500"
                                    >
                                        <div className="flex items-center space-x-2">
                                            {transfer.direction === 'upload' ? (
                                                <Upload className="w-4 h-4 text-yellow-400 flex-shrink-0" />
                                            ) : (
                                                <Download className="w-4 h-4 text-yellow-400 flex-shrink-0" />
                                            )}
                                            <div className="min-w-0 flex-1">
                                                <div className="flex items-center space-x-2">
                                                    <p className="text-white text-sm truncate">{transfer.fileName}</p>
                                                    {getFileSizeBadge(transfer.fileSize)}
                                                </div>
                                                <p className="text-gray-400 text-xs">
                                                    {transfer.direction === 'upload' ? 'to' : 'from'} {transfer.peerName} • {formatBytes(transfer.fileSize)}
                                                </p>
                                            </div>
                                            <span className="text-yellow-400 text-xs font-medium">QUEUED</span>
                                        </div>
                                    </motion.div>
                                ))}
                            </div>
                        )}

                        {/* Active Transfers */}
                        {activeTransfers.length > 0 && (
                            <div className="p-3 space-y-3">
                                <h4 className="text-xs font-medium text-gray-400 uppercase tracking-wide flex items-center space-x-2">
                                    <Activity className="w-3 h-3" />
                                    <span>Active ({activeTransfers.length})</span>
                                    {activeTransfers.some(t => t.isStreaming) && (
                                        <div className="flex items-center space-x-1">
                                            <Zap className="w-3 h-3 text-green-400" />
                                            <span className="text-green-400 text-xs">STREAMING</span>
                                        </div>
                                    )}
                                </h4>
                                {activeTransfers.map((transfer) => (
                                    <TransferCard key={transfer.id} transfer={transfer} />
                                ))}
                            </div>
                        )}

                        {/* Recent Transfers */}
                        {completedTransfers.length > 0 && (
                            <div className="p-3 border-t border-white/10 space-y-2">
                                <h4 className="text-xs font-medium text-gray-400 uppercase tracking-wide">
                                    Recent
                                </h4>
                                {completedTransfers.slice(0, 5).map((transfer) => (
                                    <motion.div
                                        key={transfer.id}
                                        layout
                                        className="flex items-center space-x-3 p-2 bg-white/5 rounded hover:bg-white/10 transition-colors"
                                    >
                                        <div className="flex-shrink-0">
                                            {transfer.status === 'completed' ? (
                                                <CheckCircle className="w-4 h-4 text-green-400" />
                                            ) : (
                                                <XCircle className="w-4 h-4 text-red-400" />
                                            )}
                                        </div>
                                        <div className="min-w-0 flex-1">
                                            <div className="flex items-center space-x-2">
                                                <p className="text-white text-sm truncate">{transfer.fileName}</p>
                                                {getFileSizeBadge(transfer.fileSize)}
                                            </div>
                                            <div className="flex items-center space-x-2 text-xs text-gray-400">
                                                <span>{transfer.status === 'completed' ? 'Completed' : 'Failed'}</span>
                                                <span>•</span>
                                                <span>{formatBytes(transfer.fileSize)}</span>
                                                {transfer.averageSpeed && transfer.status === 'completed' && (
                                                    <>
                                                        <span>•</span>
                                                        <span>avg {formatSpeed(transfer.averageSpeed)}</span>
                                                    </>
                                                )}
                                            </div>
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