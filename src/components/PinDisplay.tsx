import React, { useEffect, useState } from 'react';
import { motion } from 'framer-motion';
import { RefreshCw, Copy, Shield, Clock } from 'lucide-react';

interface PinDisplayProps {
    pin: string;
    expiresAt: number;
    onRefresh: () => void;
}

const PinDisplay: React.FC<PinDisplayProps> = ({ pin, expiresAt, onRefresh }) => {
    const [timeRemaining, setTimeRemaining] = useState<number>(0);
    const [copied, setCopied] = useState(false);

    useEffect(() => {
        const interval = setInterval(() => {
            const now = Math.floor(Date.now() / 1000);
            const remaining = Math.max(0, expiresAt - now);
            setTimeRemaining(remaining);

            // Auto-refresh when expired
            if (remaining === 0) {
                onRefresh();
            }
        }, 1000);

        return () => clearInterval(interval);
    }, [expiresAt, onRefresh]);

    const handleCopyPin = async () => {
        try {
            await navigator.clipboard.writeText(pin);
            setCopied(true);
            setTimeout(() => setCopied(false), 2000);
        } catch (err) {
            console.error('Failed to copy PIN:', err);
        }
    };

    const formatTime = (seconds: number) => {
        const mins = Math.floor(seconds / 60);
        const secs = seconds % 60;
        return `${mins}:${secs.toString().padStart(2, '0')}`;
    };

    const getProgressPercentage = () => {
        // Assuming PIN lifetime is 120 seconds (2 minutes)
        const totalTime = 120;
        return (timeRemaining / totalTime) * 100;
    };

    // Format PIN with dash in the middle
    const formattedPin = pin.length === 6 ? `${pin.slice(0, 3)}-${pin.slice(3)}` : pin;

    return (
        <div className="bg-gradient-to-br from-blue-500/10 to-purple-500/10 backdrop-blur-md rounded-2xl p-8 border border-white/20 shadow-2xl">
            <div className="flex items-center justify-between mb-6">
                <div className="flex items-center space-x-2">
                    <Shield className="w-5 h-5 text-blue-400" />
                    <h3 className="text-lg font-semibold text-white">Pairing PIN</h3>
                </div>
                <div className="flex items-center space-x-2">
                    <Clock className="w-4 h-4 text-gray-400" />
                    <span className="text-sm text-gray-400">{formatTime(timeRemaining)}</span>
                </div>
            </div>

            {/* PIN Display */}
            <div className="relative mb-6">
                <motion.div
                    initial={{ scale: 0.95, opacity: 0 }}
                    animate={{ scale: 1, opacity: 1 }}
                    transition={{ duration: 0.3 }}
                    className="bg-black/30 rounded-xl p-6 text-center"
                >
                    <div className="text-5xl font-bold text-white tracking-wider font-mono">
                        {formattedPin}
                    </div>
                </motion.div>

                {/* Progress Ring */}
                <svg className="absolute top-0 right-0 w-20 h-20 -mt-2 -mr-2">
                    <circle
                        cx="40"
                        cy="40"
                        r="36"
                        stroke="rgba(255, 255, 255, 0.1)"
                        strokeWidth="4"
                        fill="none"
                    />
                    <motion.circle
                        cx="40"
                        cy="40"
                        r="36"
                        stroke="url(#gradient)"
                        strokeWidth="4"
                        fill="none"
                        strokeDasharray={226}
                        strokeDashoffset={226 - (226 * getProgressPercentage()) / 100}
                        strokeLinecap="round"
                        initial={{ strokeDashoffset: 226 }}
                        animate={{ strokeDashoffset: 226 - (226 * getProgressPercentage()) / 100 }}
                        transition={{ duration: 1, ease: "linear" }}
                    />
                    <defs>
                        <linearGradient id="gradient" x1="0%" y1="0%" x2="100%" y2="100%">
                            <stop offset="0%" stopColor="#3b82f6" />
                            <stop offset="100%" stopColor="#a855f7" />
                        </linearGradient>
                    </defs>
                </svg>
            </div>

            {/* Actions */}
            <div className="flex space-x-3">
                <motion.button
                    whileHover={{ scale: 1.05 }}
                    whileTap={{ scale: 0.95 }}
                    onClick={handleCopyPin}
                    className={`flex-1 py-3 px-4 rounded-lg font-medium transition-all flex items-center justify-center space-x-2 ${
                        copied
                            ? 'bg-green-500/20 text-green-400 border border-green-400/30'
                            : 'bg-white/10 text-white hover:bg-white/20 border border-white/20'
                    }`}
                >
                    <Copy className="w-4 h-4" />
                    <span>{copied ? 'Copied!' : 'Copy PIN'}</span>
                </motion.button>

                <motion.button
                    whileHover={{ scale: 1.05 }}
                    whileTap={{ scale: 0.95 }}
                    onClick={onRefresh}
                    className="py-3 px-4 rounded-lg bg-blue-500/20 text-blue-400 hover:bg-blue-500/30 font-medium transition-all border border-blue-400/30 flex items-center space-x-2"
                >
                    <RefreshCw className="w-4 h-4" />
                    <span>Refresh</span>
                </motion.button>
            </div>

            {/* Info Text */}
            <p className="text-xs text-gray-400 text-center mt-4">
                Share this PIN with the device you want to pair with
            </p>
        </div>
    );
};

export default PinDisplay;