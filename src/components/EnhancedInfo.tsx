import React, { useState, useEffect } from 'react';
import {
    Keyboard,
    Monitor,
    Cpu,
    HardDrive,
    Wifi,
    Activity,
    Clock,
    Download,
    Upload,
    Users,
    Globe,
    Shield,
    Zap
} from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';

interface SystemInfo {
    platform: string;
    arch: string;
    version: string;
    memory: number;
    uptime: number;
}

interface NetworkMetrics {
    bytesReceived: number;
    bytesSent: number;
    transfersTotal: number;
    avgSpeed: number;
}

const EnhancedInfo: React.FC = () => {
    const [systemInfo, setSystemInfo] = useState<SystemInfo | null>(null);
    const [networkMetrics, setNetworkMetrics] = useState<NetworkMetrics | null>(null);
    const [isLoading, setIsLoading] = useState(true);

    useEffect(() => {
        loadSystemInfo();
        loadNetworkMetrics();

        // Refresh metrics every 10 seconds
        const interval = setInterval(() => {
            loadNetworkMetrics();
        }, 10000);

        return () => clearInterval(interval);
    }, []);

    const loadSystemInfo = async () => {
        try {
            const info = await invoke<SystemInfo>('get_system_info');
            setSystemInfo(info);
        } catch (error) {
            console.error('Failed to load system info:', error);
        } finally {
            setIsLoading(false);
        }
    };

    const loadNetworkMetrics = async () => {
        try {
            const metrics = await invoke<NetworkMetrics>('get_network_metrics');
            setNetworkMetrics(metrics);
        } catch (error) {
            console.error('Failed to load network metrics:', error);
        }
    };

    const formatBytes = (bytes: number) => {
        if (bytes === 0) return '0 B';
        const k = 1024;
        const sizes = ['B', 'KB', 'MB', 'GB', 'TB'];
        const i = Math.floor(Math.log(bytes) / Math.log(k));
        return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
    };

    const formatUptime = (seconds: number) => {
        const days = Math.floor(seconds / 86400);
        const hours = Math.floor((seconds % 86400) / 3600);
        const minutes = Math.floor((seconds % 3600) / 60);

        if (days > 0) return `${days}d ${hours}h`;
        if (hours > 0) return `${hours}h ${minutes}m`;
        return `${minutes}m`;
    };

    const getPlatformHotkeys = () => {
        if (!systemInfo) return { copy: 'Loading...', paste: 'Loading...' };

        switch (systemInfo.platform.toLowerCase()) {
            case 'darwin':
                return { copy: '⌘⇧Y', paste: '⌘⇧I' };
            case 'windows':
                return { copy: 'Ctrl+Alt+Y', paste: 'Ctrl+Alt+I' };
            default:
                return { copy: 'Ctrl+Shift+Y', paste: 'Ctrl+Shift+I' };
        }
    };

    const hotkeys = getPlatformHotkeys();

    if (isLoading) {
        return (
            <div className="flex items-center justify-center py-8">
                <div className="w-8 h-8 border-2 border-blue-400 border-t-transparent rounded-full animate-spin"></div>
            </div>
        );
    }

    return (
        <div className="space-y-4">
            {/* Hotkeys */}
            <div className="bg-blue-500/10 border border-blue-500/20 rounded-lg p-4">
                <h3 className="text-blue-300 font-medium mb-3 flex items-center">
                    <Keyboard className="w-4 h-4 mr-2" />
                    Hotkeys
                </h3>
                <div className="space-y-2">
                    <div className="flex justify-between items-center">
                        <span className="text-gray-300 text-sm">Copy file:</span>
                        <kbd className="bg-black/30 px-2 py-1 rounded text-xs text-white font-mono">
                            {hotkeys.copy}
                        </kbd>
                    </div>
                    <div className="flex justify-between items-center">
                        <span className="text-gray-300 text-sm">Paste file:</span>
                        <kbd className="bg-black/30 px-2 py-1 rounded text-xs text-white font-mono">
                            {hotkeys.paste}
                        </kbd>
                    </div>
                </div>
            </div>

            {/* System Information */}
            <div className="bg-green-500/10 border border-green-500/20 rounded-lg p-4">
                <h3 className="text-green-300 font-medium mb-3 flex items-center">
                    <Monitor className="w-4 h-4 mr-2" />
                    System Information
                </h3>
                {systemInfo && (
                    <div className="space-y-2 text-sm">
                        <div className="flex justify-between">
                            <span className="text-gray-300 flex items-center">
                                <Globe className="w-3 h-3 mr-1" />
                                Platform:
                            </span>
                            <span className="text-white">{systemInfo.platform}</span>
                        </div>
                        <div className="flex justify-between">
                            <span className="text-gray-300 flex items-center">
                                <Cpu className="w-3 h-3 mr-1" />
                                Architecture:
                            </span>
                            <span className="text-white">{systemInfo.arch}</span>
                        </div>
                        <div className="flex justify-between">
                            <span className="text-gray-300 flex items-center">
                                <HardDrive className="w-3 h-3 mr-1" />
                                Memory:
                            </span>
                            <span className="text-white">{formatBytes(systemInfo.memory)}</span>
                        </div>
                        <div className="flex justify-between">
                            <span className="text-gray-300 flex items-center">
                                <Clock className="w-3 h-3 mr-1" />
                                Uptime:
                            </span>
                            <span className="text-white">{formatUptime(systemInfo.uptime)}</span>
                        </div>
                    </div>
                )}
            </div>

            {/* Network Metrics */}
            <div className="bg-purple-500/10 border border-purple-500/20 rounded-lg p-4">
                <h3 className="text-purple-300 font-medium mb-3 flex items-center">
                    <Activity className="w-4 h-4 mr-2" />
                    Network Activity
                </h3>
                {networkMetrics && (
                    <div className="space-y-2 text-sm">
                        <div className="flex justify-between">
                            <span className="text-gray-300 flex items-center">
                                <Download className="w-3 h-3 mr-1" />
                                Downloaded:
                            </span>
                            <span className="text-green-400">{formatBytes(networkMetrics.bytesReceived)}</span>
                        </div>
                        <div className="flex justify-between">
                            <span className="text-gray-300 flex items-center">
                                <Upload className="w-3 h-3 mr-1" />
                                Uploaded:
                            </span>
                            <span className="text-blue-400">{formatBytes(networkMetrics.bytesSent)}</span>
                        </div>
                        <div className="flex justify-between">
                            <span className="text-gray-300 flex items-center">
                                <Users className="w-3 h-3 mr-1" />
                                Total transfers:
                            </span>
                            <span className="text-white">{networkMetrics.transfersTotal}</span>
                        </div>
                        <div className="flex justify-between">
                            <span className="text-gray-300 flex items-center">
                                <Zap className="w-3 h-3 mr-1" />
                                Avg speed:
                            </span>
                            <span className="text-yellow-400">{formatBytes(networkMetrics.avgSpeed)}/s</span>
                        </div>
                    </div>
                )}
            </div>

            {/* App Information */}
            <div className="bg-gray-500/10 border border-gray-500/20 rounded-lg p-4">
                <h3 className="text-gray-300 font-medium mb-3 flex items-center">
                    <Shield className="w-4 h-4 mr-2" />
                    Application
                </h3>
                <div className="space-y-2 text-sm">
                    <div className="flex justify-between">
                        <span className="text-gray-300">Version:</span>
                        <span className="text-white">0.1.0</span>
                    </div>
                    <div className="flex justify-between">
                        <span className="text-gray-300">Build:</span>
                        <span className="text-white">Enhanced Device Management</span>
                    </div>
                    <div className="flex justify-between">
                        <span className="text-gray-300">Protocol:</span>
                        <span className="text-white">Fileshare v2</span>
                    </div>
                </div>
            </div>
        </div>
    );
};

export default EnhancedInfo;