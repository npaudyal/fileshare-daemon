import React from 'react';
import { motion } from 'framer-motion';
import { Wifi, RefreshCw, CheckSquare, Download } from 'lucide-react';

interface HeaderProps {
    connectionStatus: boolean;
    isLoading: boolean;
    isRefreshing: boolean;
    lastUpdate: Date;
    filteredDevicesCount: number;
    deviceName?: string;
    showTransferProgress?: boolean;
    onRefresh: () => void;
    onSelectAll: () => void;
    onToggleTransferProgress?: () => void;
}

const Header: React.FC<HeaderProps> = ({
    connectionStatus,
    isLoading,
    isRefreshing,
    lastUpdate,
    filteredDevicesCount,
    deviceName,
    showTransferProgress,
    onRefresh,
    onSelectAll,
    onToggleTransferProgress
}) => {
    return (
        <div className="p-4 border-b border-white/10 bg-slate-900/50">
            <div className="flex items-center justify-between">
                <div className="flex items-center space-x-2">
                    <div className="relative">
                        <motion.div
                            animate={{ rotate: connectionStatus ? 0 : 0 }}
                            transition={{ duration: 0.3 }}
                        >
                            <Wifi className={`w-5 h-5 ${connectionStatus ? 'text-green-400' : 'text-gray-400'}`} />
                        </motion.div>
                        {connectionStatus && (
                            <motion.div
                                initial={{ scale: 0 }}
                                animate={{ scale: 1 }}
                                className="absolute -top-1 -right-1 w-2 h-2 bg-green-400 rounded-full animate-pulse"
                            />
                        )}
                    </div>
                    <h1 className="text-white font-semibold">Fileshare</h1>
                    {isLoading && (
                        <motion.div
                            initial={{ scale: 0 }}
                            animate={{ scale: 1 }}
                            exit={{ scale: 0 }}
                            className="w-3 h-3 border border-blue-400 border-t-transparent rounded-full animate-spin"
                        />
                    )}
                </div>
                <div className="flex items-center space-x-3">
                    {onToggleTransferProgress && (
                        <motion.button
                            whileHover={{ scale: 1.05 }}
                            whileTap={{ scale: 0.95 }}
                            onClick={onToggleTransferProgress}
                            className={`px-3 py-1 rounded-md transition-all duration-200 ${
                                showTransferProgress 
                                    ? 'text-blue-300 bg-blue-500/30 border border-blue-400/50' 
                                    : 'text-gray-400 hover:text-white hover:bg-white/10 border border-white/10'
                            }`}
                            title="Toggle Transfer Progress"
                        >
                            <div className="flex items-center space-x-1">
                                <Download className="w-4 h-4" />
                                <span className="text-xs font-medium">Transfers</span>
                            </div>
                        </motion.button>
                    )}
                    <motion.button
                        whileHover={{ scale: 1.1, rotate: 180 }}
                        whileTap={{ scale: 0.9 }}
                        onClick={onRefresh}
                        disabled={isRefreshing}
                        className="text-gray-400 hover:text-white p-1 rounded hover:bg-white/10 transition-colors"
                        title="Refresh Devices"
                    >
                        <RefreshCw className={`w-4 h-4 ${isRefreshing ? 'animate-spin' : ''}`} />
                    </motion.button>
                    {filteredDevicesCount > 1 && (
                        <motion.button
                            whileHover={{ scale: 1.1 }}
                            whileTap={{ scale: 0.9 }}
                            onClick={onSelectAll}
                            className="text-gray-400 hover:text-white p-1 rounded hover:bg-white/10 transition-colors"
                            title="Select All Devices"
                        >
                            <CheckSquare className="w-4 h-4" />
                        </motion.button>
                    )}
                    {deviceName && (
                        <span className="text-xs text-gray-400">{deviceName}</span>
                    )}
                </div>
            </div>
            <motion.div
                initial={{ opacity: 0 }}
                animate={{ opacity: 1 }}
                transition={{ delay: 0.2 }}
                className="mt-2 text-xs text-gray-500"
            >
                Last updated: {lastUpdate.toLocaleTimeString()}
            </motion.div>
        </div>
    );
};

export default Header;