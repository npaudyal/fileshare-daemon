import React, { useState } from 'react';
import { motion } from 'framer-motion';
import { useTheme } from '../../context/ThemeContext';
import { TransferInfo } from './types';
import {
    Download,
    Upload,
    Pause,
    Play,
    X,
    Clock,
    AlertCircle,
    CheckCircle,
    MoreHorizontal,
    ChevronUp,
    Minus
} from 'lucide-react';

interface TransferItemProps {
    transfer: TransferInfo;
    onPauseResume: (id: string) => void;
    onCancel: (id: string) => void;
    onSetPriority: (id: string, priority: number) => void;
}

const TransferItem: React.FC<TransferItemProps> = ({
    transfer,
    onPauseResume,
    onCancel,
    onSetPriority
}) => {
    const { theme } = useTheme();
    const [showActions, setShowActions] = useState(false);
    const [hoveredButton, setHoveredButton] = useState<string | null>(null);

    const formatBytes = (bytes: number) => {
        if (bytes === 0) return '0 B';
        const k = 1024;
        const sizes = ['B', 'KB', 'MB', 'GB'];
        const i = Math.floor(Math.log(bytes) / Math.log(k));
        return `${(bytes / Math.pow(k, i)).toFixed(1)} ${sizes[i]}`;
    };

    const formatSpeed = (bytesPerSecond: number) => {
        return formatBytes(bytesPerSecond) + '/s';
    };

    const formatTime = (seconds: number) => {
        if (seconds === 0 || !isFinite(seconds)) return '∞';
        if (seconds < 60) return `${Math.round(seconds)}s`;
        if (seconds < 3600) return `${Math.round(seconds / 60)}m`;
        return `${Math.round(seconds / 3600)}h`;
    };

    const getStatusColor = (state: string) => {
        switch (state) {
            case 'Active': return theme.colors.success;
            case 'Paused': return theme.colors.warning;
            case 'Completed': return theme.colors.success;
            case 'Failed': return theme.colors.error;
            case 'Connecting': return theme.colors.accent2;
            case 'Negotiating': return theme.colors.accent2;
            default: return theme.colors.textSecondary;
        }
    };

    const getStatusIcon = (state: string) => {
        switch (state) {
            case 'Active':
                return transfer.direction === 'Download' ?
                    <Download className="w-4 h-4" /> :
                    <Upload className="w-4 h-4" />;
            case 'Paused':
                return <Pause className="w-4 h-4" />;
            case 'Completed':
                return <CheckCircle className="w-4 h-4" />;
            case 'Failed':
                return <AlertCircle className="w-4 h-4" />;
            case 'Connecting':
            case 'Negotiating':
                return <Clock className="w-4 h-4" />;
            default:
                return <Minus className="w-4 h-4" />;
        }
    };

    const getPriorityColor = (priority: number) => {
        switch (priority) {
            case 0: return theme.colors.textSecondary; // Low
            case 1: return theme.colors.text; // Normal
            case 2: return theme.colors.warning; // High
            case 3: return theme.colors.error; // Critical
            default: return theme.colors.text;
        }
    };

    const isActive = transfer.state === 'Active';
    const isPaused = transfer.state === 'Paused';
    const canPause = isActive || isPaused;
    const canCancel = !['Completed', 'Failed', 'Cancelled'].includes(transfer.state);

    const eta = transfer.eta ? formatTime(transfer.eta) : null;
    const progressBytes = formatBytes(transfer.bytesTransferred);
    const totalBytes = formatBytes(transfer.fileSize);

    return (
        <motion.div
            layout
            className="rounded-lg p-3 space-y-3 border"
            style={{
                backgroundColor: theme.colors.backgroundSecondary,
                borderColor: theme.colors.border
            }}
            onMouseEnter={() => setShowActions(true)}
            onMouseLeave={() => setShowActions(false)}
        >
            {/* Header */}
            <div className="flex items-center justify-between">
                <div className="flex items-center space-x-2 min-w-0 flex-1">
                    <div style={{ color: getStatusColor(transfer.state) }}>
                        {getStatusIcon(transfer.state)}
                    </div>

                    <div className="min-w-0 flex-1">
                        <div className="flex items-center space-x-2">
                            <p className="text-sm font-medium truncate" style={{ color: theme.colors.text }}>
                                {transfer.fileName}
                            </p>

                            {(transfer.priority ?? 0) > 1 && (
                                <div className="flex">
                                    {Array.from({ length: (transfer.priority ?? 0) - 1 }).map((_, i) => (
                                        <ChevronUp
                                            key={i}
                                            className="w-3 h-3"
                                            style={{ color: getPriorityColor(transfer.priority ?? 0) }}
                                        />
                                    ))}
                                </div>
                            )}
                        </div>

                        <p className="text-xs" style={{ color: theme.colors.textSecondary }}>
                            {transfer.direction === 'Download' ? 'from' : 'to'} {transfer.peerName}
                        </p>
                    </div>
                </div>

                {/* Action Buttons */}
                <motion.div
                    className="flex items-center space-x-1"
                    initial={{ opacity: 0, x: 10 }}
                    animate={{ opacity: showActions ? 1 : 0, x: showActions ? 0 : 10 }}
                    transition={{ duration: 0.2 }}
                >
                    {canPause && (
                        <button
                            onClick={() => onPauseResume(transfer.id)}
                            onMouseEnter={() => setHoveredButton(`pause-${transfer.id}`)}
                            onMouseLeave={() => setHoveredButton(null)}
                            className="p-1 rounded transition-colors"
                            style={{
                                color: hoveredButton === `pause-${transfer.id}` ? theme.colors.text : theme.colors.textSecondary,
                                backgroundColor: hoveredButton === `pause-${transfer.id}` ? theme.colors.backgroundTertiary : 'transparent'
                            }}
                        >
                            {isPaused ? <Play className="w-3 h-3" /> : <Pause className="w-3 h-3" />}
                        </button>
                    )}

                    {canCancel && (
                        <button
                            onClick={() => onCancel(transfer.id)}
                            onMouseEnter={() => setHoveredButton(`cancel-${transfer.id}`)}
                            onMouseLeave={() => setHoveredButton(null)}
                            className="p-1 rounded transition-colors"
                            style={{
                                color: hoveredButton === `cancel-${transfer.id}` ? theme.colors.error : theme.colors.textSecondary,
                                backgroundColor: hoveredButton === `cancel-${transfer.id}` ? theme.colors.backgroundTertiary : 'transparent'
                            }}
                        >
                            <X className="w-3 h-3" />
                        </button>
                    )}

                    <button
                        onClick={() => setShowActions(!showActions)}
                        onMouseEnter={() => setHoveredButton(`more-${transfer.id}`)}
                        onMouseLeave={() => setHoveredButton(null)}
                        className="p-1 rounded transition-colors"
                        style={{
                            color: hoveredButton === `more-${transfer.id}` ? theme.colors.text : theme.colors.textSecondary,
                            backgroundColor: hoveredButton === `more-${transfer.id}` ? theme.colors.backgroundTertiary : 'transparent'
                        }}
                    >
                        <MoreHorizontal className="w-3 h-3" />
                    </button>
                </motion.div>
            </div>

            {/* Progress Section */}
            {(isActive || isPaused) && (
                <div className="space-y-2">
                    {/* Progress Bar */}
                    <div
                        className="w-full rounded-full h-2"
                        style={{ backgroundColor: theme.colors.backgroundTertiary }}
                    >
                        <motion.div
                            className="h-2 rounded-full relative overflow-hidden"
                            style={{ backgroundColor: getStatusColor(transfer.state) }}
                            initial={{ width: 0 }}
                            animate={{ width: `${transfer.progress}%` }}
                            transition={{ duration: 0.3 }}
                        >
                            {/* Animated progress effect */}
                            {isActive && (
                                <motion.div
                                    className="absolute inset-0 bg-white opacity-20"
                                    animate={{ x: ['-100%', '100%'] }}
                                    transition={{ repeat: Infinity, duration: 1.5, ease: 'linear' }}
                                />
                            )}
                        </motion.div>
                    </div>

                    {/* Progress Details */}
                    <div className="flex justify-between items-center text-xs">
                        <div className="flex items-center space-x-3">
                            <span style={{ color: getStatusColor(transfer.state) }}>
                                {transfer.progress.toFixed(1)}%
                            </span>

                            <span style={{ color: theme.colors.textSecondary }}>
                                {progressBytes} / {totalBytes}
                            </span>
                        </div>

                        <div className="flex items-center space-x-3" style={{ color: theme.colors.textSecondary }}>
                            {isActive && transfer.speed > 0 && (
                                <span>{formatSpeed(transfer.speed)}</span>
                            )}

                            {eta && isActive && (
                                <span>{eta} remaining</span>
                            )}

                            {isPaused && (
                                <span className="text-xs px-2 py-0.5 rounded" style={{
                                    backgroundColor: theme.colors.warning + '20',
                                    color: theme.colors.warning
                                }}>
                                    Paused
                                </span>
                            )}
                        </div>
                    </div>
                </div>
            )}

            {/* Error Message */}
            {transfer.state === 'Failed' && transfer.error && (
                <div
                    className="text-xs p-2 rounded"
                    style={{
                        backgroundColor: theme.colors.error + '10',
                        color: theme.colors.error
                    }}
                >
                    {transfer.error}
                </div>
            )}

            {/* Priority Actions */}
            {showActions && canCancel && (
                <motion.div
                    initial={{ opacity: 0, height: 0 }}
                    animate={{ opacity: 1, height: 'auto' }}
                    exit={{ opacity: 0, height: 0 }}
                    className="pt-2 border-t"
                    style={{ borderColor: theme.colors.border }}
                >
                    <div className="flex items-center justify-between">
                        <span className="text-xs" style={{ color: theme.colors.textSecondary }}>
                            Priority
                        </span>

                        <div className="flex items-center space-x-1">
                            {[0, 1, 2, 3].map((priority) => (
                                <button
                                    key={priority}
                                    onClick={() => onSetPriority(transfer.id, priority)}
                                    className="px-2 py-1 text-xs rounded transition-colors"
                                    style={{
                                        backgroundColor: transfer.priority === priority
                                            ? getPriorityColor(priority) + '20'
                                            : 'transparent',
                                        color: transfer.priority === priority
                                            ? getPriorityColor(priority)
                                            : theme.colors.textSecondary,
                                        border: `1px solid ${transfer.priority === priority
                                            ? getPriorityColor(priority)
                                            : theme.colors.border}`
                                    }}
                                >
                                    {['Low', 'Normal', 'High', 'Critical'][priority]}
                                </button>
                            ))}
                        </div>
                    </div>
                </motion.div>
            )}
        </motion.div>
    );
};

export default TransferItem;