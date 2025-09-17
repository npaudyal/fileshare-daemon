import React, { useState, useEffect } from 'react';
import {
    Keyboard,
    Monitor,
    Cpu,
    HardDrive,
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
import { useTheme } from '../context/ThemeContext';

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
    const { theme } = useTheme();
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
                <div
                    className="w-8 h-8 border-2 border-t-transparent rounded-full animate-spin"
                    style={{
                        borderColor: theme.colors.accent2,
                        borderTopColor: 'transparent'
                    }}
                ></div>
            </div>
        );
    }

    return (
        <div className="space-y-4">
            {/* Hotkeys */}
            <div
                className="rounded-lg p-4 border"
                style={{
                    backgroundColor: theme.colors.accent2 + '10',
                    borderColor: theme.colors.accent2 + '30'
                }}
            >
                <h3 className="font-medium mb-3 flex items-center" style={{ color: theme.colors.accent2 }}>
                    <Keyboard className="w-4 h-4 mr-2" />
                    Hotkeys
                </h3>
                <div className="space-y-2">
                    <div className="flex justify-between items-center">
                        <span className="text-sm" style={{ color: theme.colors.textSecondary }}>Copy file:</span>
                        <kbd
                            className="px-2 py-1 rounded text-xs font-mono"
                            style={{
                                backgroundColor: theme.colors.backgroundTertiary,
                                color: theme.colors.text
                            }}
                        >
                            {hotkeys.copy}
                        </kbd>
                    </div>
                    <div className="flex justify-between items-center">
                        <span className="text-sm" style={{ color: theme.colors.textSecondary }}>Paste file:</span>
                        <kbd
                            className="px-2 py-1 rounded text-xs font-mono"
                            style={{
                                backgroundColor: theme.colors.backgroundTertiary,
                                color: theme.colors.text
                            }}
                        >
                            {hotkeys.paste}
                        </kbd>
                    </div>
                </div>
            </div>

            {/* System Information */}
            <div
                className="rounded-lg p-4 border"
                style={{
                    backgroundColor: theme.colors.success + '10',
                    borderColor: theme.colors.success + '30'
                }}
            >
                <h3 className="font-medium mb-3 flex items-center" style={{ color: theme.colors.success }}>
                    <Monitor className="w-4 h-4 mr-2" />
                    System Information
                </h3>
                {systemInfo && (
                    <div className="space-y-2 text-sm">
                        <div className="flex justify-between">
                            <span className="flex items-center" style={{ color: theme.colors.textSecondary }}>
                                <Globe className="w-3 h-3 mr-1" />
                                Platform:
                            </span>
                            <span style={{ color: theme.colors.text }}>{systemInfo.platform}</span>
                        </div>
                        <div className="flex justify-between">
                            <span className="flex items-center" style={{ color: theme.colors.textSecondary }}>
                                <Cpu className="w-3 h-3 mr-1" />
                                Architecture:
                            </span>
                            <span style={{ color: theme.colors.text }}>{systemInfo.arch}</span>
                        </div>
                        <div className="flex justify-between">
                            <span className="flex items-center" style={{ color: theme.colors.textSecondary }}>
                                <HardDrive className="w-3 h-3 mr-1" />
                                Memory:
                            </span>
                            <span style={{ color: theme.colors.text }}>{formatBytes(systemInfo.memory)}</span>
                        </div>
                        <div className="flex justify-between">
                            <span className="flex items-center" style={{ color: theme.colors.textSecondary }}>
                                <Clock className="w-3 h-3 mr-1" />
                                Uptime:
                            </span>
                            <span style={{ color: theme.colors.text }}>{formatUptime(systemInfo.uptime)}</span>
                        </div>
                    </div>
                )}
            </div>

            {/* Network Metrics */}
            <div
                className="rounded-lg p-4 border"
                style={{
                    backgroundColor: theme.colors.hover + '10',
                    borderColor: theme.colors.hover + '30'
                }}
            >
                <h3 className="font-medium mb-3 flex items-center" style={{ color: theme.colors.hover }}>
                    <Activity className="w-4 h-4 mr-2" />
                    Network Activity
                </h3>
                {networkMetrics && (
                    <div className="space-y-2 text-sm">
                        <div className="flex justify-between">
                            <span className="flex items-center" style={{ color: theme.colors.textSecondary }}>
                                <Download className="w-3 h-3 mr-1" />
                                Downloaded:
                            </span>
                            <span style={{ color: theme.colors.success }}>{formatBytes(networkMetrics.bytesReceived)}</span>
                        </div>
                        <div className="flex justify-between">
                            <span className="flex items-center" style={{ color: theme.colors.textSecondary }}>
                                <Upload className="w-3 h-3 mr-1" />
                                Uploaded:
                            </span>
                            <span style={{ color: theme.colors.accent2 }}>{formatBytes(networkMetrics.bytesSent)}</span>
                        </div>
                        <div className="flex justify-between">
                            <span className="flex items-center" style={{ color: theme.colors.textSecondary }}>
                                <Users className="w-3 h-3 mr-1" />
                                Total transfers:
                            </span>
                            <span style={{ color: theme.colors.text }}>{networkMetrics.transfersTotal}</span>
                        </div>
                        <div className="flex justify-between">
                            <span className="flex items-center" style={{ color: theme.colors.textSecondary }}>
                                <Zap className="w-3 h-3 mr-1" />
                                Avg speed:
                            </span>
                            <span style={{ color: theme.colors.warning }}>{formatBytes(networkMetrics.avgSpeed)}/s</span>
                        </div>
                    </div>
                )}
            </div>

            {/* App Information */}
            <div
                className="rounded-lg p-4 border"
                style={{
                    backgroundColor: theme.colors.textSecondary + '10',
                    borderColor: theme.colors.textSecondary + '30'
                }}
            >
                <h3 className="font-medium mb-3 flex items-center" style={{ color: theme.colors.textSecondary }}>
                    <Shield className="w-4 h-4 mr-2" />
                    Application
                </h3>
                <div className="space-y-2 text-sm">
                    <div className="flex justify-between">
                        <span style={{ color: theme.colors.textSecondary }}>Version:</span>
                        <span style={{ color: theme.colors.text }}>0.1.0</span>
                    </div>
                    <div className="flex justify-between">
                        <span style={{ color: theme.colors.textSecondary }}>Build:</span>
                        <span style={{ color: theme.colors.text }}>Enhanced Device Management</span>
                    </div>
                    <div className="flex justify-between">
                        <span style={{ color: theme.colors.textSecondary }}>Protocol:</span>
                        <span style={{ color: theme.colors.text }}>Fileshare v2</span>
                    </div>
                </div>
            </div>
        </div>
    );
};

export default EnhancedInfo;