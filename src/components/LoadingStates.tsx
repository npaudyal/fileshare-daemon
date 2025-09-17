import React from 'react';
import { motion } from 'framer-motion';
import { useTheme } from '../context/ThemeContext';

export const SkeletonLoader: React.FC<{ className?: string }> = ({ className = '' }) => {
    const { theme } = useTheme();
    return (
        <motion.div
            className={`rounded animate-pulse ${className}`}
            style={{ backgroundColor: theme.colors.backgroundTertiary + '50' }}
            animate={{ opacity: [0.3, 0.6, 0.3] }}
            transition={{ duration: 1.5, repeat: Infinity }}
        />
    );
};

export const DeviceCardSkeleton: React.FC = () => {
    const { theme } = useTheme();
    return (
        <div
            className="backdrop-blur-sm rounded-lg p-4 border space-y-3"
            style={{
                backgroundColor: theme.colors.backgroundSecondary + '80',
                borderColor: theme.colors.border
            }}
        >
            <div className="flex items-center space-x-3">
                <SkeletonLoader className="w-6 h-6 rounded" />
                <div className="flex-1 space-y-2">
                    <SkeletonLoader className="h-4 w-3/4" />
                    <SkeletonLoader className="h-3 w-1/2" />
                </div>
            </div>
            <div className="flex justify-between">
                <SkeletonLoader className="h-6 w-16 rounded" />
                <SkeletonLoader className="h-6 w-20 rounded" />
            </div>
        </div>
    );
};

export const SpinningLoader: React.FC<{ size?: 'sm' | 'md' | 'lg' }> = ({ size = 'md' }) => {
    const { theme } = useTheme();
    const sizeClasses = {
        sm: 'w-4 h-4',
        md: 'w-6 h-6',
        lg: 'w-8 h-8'
    };

    return (
        <motion.div
            className={`border-2 border-t-transparent rounded-full ${sizeClasses[size]}`}
            style={{
                borderColor: theme.colors.accent2,
                borderTopColor: 'transparent'
            }}
            animate={{ rotate: 360 }}
            transition={{ duration: 1, repeat: Infinity, ease: 'linear' }}
        />
    );
};

export const PulsingDot: React.FC<{ color?: string }> = ({ color = 'bg-green-400' }) => (
    <motion.div
        className={`w-2 h-2 ${color} rounded-full`}
        animate={{ scale: [1, 1.2, 1], opacity: [1, 0.7, 1] }}
        transition={{ duration: 1.5, repeat: Infinity }}
    />
);

export const LoadingOverlay: React.FC<{ isVisible: boolean; message?: string }> = ({
    isVisible,
    message = 'Loading...'
}) => {
    const { theme } = useTheme();
    if (!isVisible) return null;

    return (
        <motion.div
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            className="absolute inset-0 flex items-center justify-center z-50"
            style={{ backgroundColor: theme.colors.background + 'C0' }}
        >
            <div
                className="rounded-lg border p-6 flex items-center space-x-3 backdrop-blur-md"
                style={{
                    backgroundColor: theme.colors.backgroundSecondary + 'F0',
                    borderColor: theme.colors.border,
                    boxShadow: `0 20px 50px ${theme.colors.shadow}`
                }}
            >
                <SpinningLoader />
                <span style={{ color: theme.colors.text }}>{message}</span>
            </div>
        </motion.div>
    );
};