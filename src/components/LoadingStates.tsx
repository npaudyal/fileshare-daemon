import React from 'react';
import { motion } from 'framer-motion';

export const SkeletonLoader: React.FC<{ className?: string }> = ({ className = '' }) => (
    <motion.div
        className={`bg-slate-700/30 rounded animate-pulse ${className}`}
        animate={{ opacity: [0.3, 0.6, 0.3] }}
        transition={{ duration: 1.5, repeat: Infinity }}
    />
);

export const DeviceCardSkeleton: React.FC = () => (
    <div className="bg-slate-800/50 backdrop-blur-sm rounded-lg p-4 border border-slate-600/30 space-y-3">
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

export const SpinningLoader: React.FC<{ size?: 'sm' | 'md' | 'lg' }> = ({ size = 'md' }) => {
    const sizeClasses = {
        sm: 'w-4 h-4',
        md: 'w-6 h-6',
        lg: 'w-8 h-8'
    };

    return (
        <motion.div
            className={`border-2 border-blue-400 border-t-transparent rounded-full ${sizeClasses[size]}`}
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
    if (!isVisible) return null;

    return (
        <motion.div
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            className="absolute inset-0 bg-slate-900/75 flex items-center justify-center z-50"
        >
            <div className="bg-slate-800 border border-slate-600/50 rounded-lg p-6 flex items-center space-x-3">
                <SpinningLoader />
                <span className="text-white">{message}</span>
            </div>
        </motion.div>
    );
};