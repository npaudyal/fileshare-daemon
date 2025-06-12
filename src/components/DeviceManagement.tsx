import React, { useState } from 'react';
import { Star, StarOff, Users, Zap, Activity, Download, Clock } from 'lucide-react';

interface DeviceStats {
    transferSpeed: number;
    successRate: number;
    lastActivity: number;
    totalData: number;
}

interface DeviceManagementProps {
    deviceId: string;
    deviceName: string;
    isFavorite: boolean;
    onToggleFavorite: (deviceId: string) => void;
    onConnectDevice: (deviceId: string) => void;
    onDisconnectDevice: (deviceId: string) => void;
    isConnected: boolean;
    stats?: DeviceStats;
}

const DeviceManagement: React.FC<DeviceManagementProps> = ({
    deviceId,
    isFavorite,
    onToggleFavorite,
    onConnectDevice,
    onDisconnectDevice,
    isConnected,
    stats
}) => {
    const [showStats, setShowStats] = useState(false);

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

    return (
        <div className="space-y-3">
            {/* Quick Actions */}
            <div className="flex items-center justify-between">
                <div className="flex items-center space-x-2">
                    <button
                        onClick={() => onToggleFavorite(deviceId)}
                        className={`p-1 rounded transition-colors ${isFavorite
                            ? 'text-yellow-400 hover:text-yellow-300'
                            : 'text-gray-400 hover:text-yellow-400'
                            }`}
                        title={isFavorite ? 'Remove from favorites' : 'Add to favorites'}
                    >
                        {isFavorite ? <Star className="w-4 h-4 fill-current" /> : <StarOff className="w-4 h-4" />}
                    </button>

                    <button
                        onClick={() => setShowStats(!showStats)}
                        className="p-1 rounded text-gray-400 hover:text-white transition-colors"
                        title="Show device statistics"
                    >
                        <Activity className="w-4 h-4" />
                    </button>
                </div>

                <div className="flex items-center space-x-2">
                    {isConnected ? (
                        <button
                            onClick={() => onDisconnectDevice(deviceId)}
                            className="px-3 py-1 bg-red-500/20 text-red-300 rounded text-xs hover:bg-red-500/30 transition-colors"
                        >
                            Disconnect
                        </button>
                    ) : (
                        <button
                            onClick={() => onConnectDevice(deviceId)}
                            className="px-3 py-1 bg-green-500/20 text-green-300 rounded text-xs hover:bg-green-500/30 transition-colors"
                        >
                            Connect
                        </button>
                    )}
                </div>
            </div>

            {/* Device Statistics */}
            {showStats && stats && (
                <div className="bg-black/20 rounded-lg p-3 border border-white/10">
                    <h4 className="text-sm font-medium text-white mb-2">Device Statistics</h4>
                    <div className="grid grid-cols-2 gap-3 text-xs">
                        <div className="flex items-center space-x-2">
                            <Zap className="w-3 h-3 text-blue-400" />
                            <span className="text-gray-400">Speed:</span>
                            <span className="text-white">{formatSpeed(stats.transferSpeed)}</span>
                        </div>

                        <div className="flex items-center space-x-2">
                            <Users className="w-3 h-3 text-green-400" />
                            <span className="text-gray-400">Success:</span>
                            <span className="text-white">{stats.successRate.toFixed(1)}%</span>
                        </div>

                        <div className="flex items-center space-x-2">
                            <Download className="w-3 h-3 text-purple-400" />
                            <span className="text-gray-400">Total:</span>
                            <span className="text-white">{formatBytes(stats.totalData)}</span>
                        </div>

                        <div className="flex items-center space-x-2">
                            <Clock className="w-3 h-3 text-yellow-400" />
                            <span className="text-gray-400">Active:</span>
                            <span className="text-white">{Math.floor((Date.now() - stats.lastActivity) / 60000)}m ago</span>
                        </div>
                    </div>
                </div>
            )}
        </div>
    );
};

export default DeviceManagement;