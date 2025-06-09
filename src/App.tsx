import React, { useState, useEffect, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import {
    Wifi,
    WifiOff,
    Smartphone,
    Laptop,
    Monitor,
    Settings,
    Info,
    Power,
    Edit2,
    Check,
    X,
    RefreshCw,
    CheckCircle,
    AlertCircle,
    Clock,
    Globe
} from 'lucide-react';

interface DeviceInfo {
    id: string;
    name: string;
    device_type: string;
    is_paired: boolean;
    is_connected: boolean;
    last_seen: number;
    address: string;
    version: string;
}

interface AppSettings {
    device_name: string;
    device_id: string;
    network_port: number;
    discovery_port: number;
    chunk_size: number;
    max_concurrent_transfers: number;
    require_pairing: boolean;
    encryption_enabled: boolean;
}

function App() {
    const [devices, setDevices] = useState<DeviceInfo[]>([]);
    const [settings, setSettings] = useState<AppSettings | null>(null);
    const [activeTab, setActiveTab] = useState<'devices' | 'settings' | 'info'>('devices');
    const [editingDevice, setEditingDevice] = useState<string | null>(null);
    const [newDeviceName, setNewDeviceName] = useState<string>('');
    const [isLoading, setIsLoading] = useState(true);
    const [lastUpdate, setLastUpdate] = useState<Date>(new Date());
    const [connectionStatus, setConnectionStatus] = useState(false);
    const [isRefreshing, setIsRefreshing] = useState(false);

    // Load devices with better error handling
    const loadDevices = useCallback(async () => {
        try {
            const discoveredDevices = await invoke<DeviceInfo[]>('get_discovered_devices');
            setDevices(discoveredDevices);
            setLastUpdate(new Date());
            console.log(`📱 Loaded ${discoveredDevices.length} devices`);
        } catch (error) {
            console.error('❌ Failed to load devices:', error);
        } finally {
            setIsLoading(false);
        }
    }, []);

    // Load settings
    const loadSettings = useCallback(async () => {
        try {
            const appSettings = await invoke<AppSettings>('get_app_settings');
            setSettings(appSettings);
            console.log('⚙️ Settings loaded');
        } catch (error) {
            console.error('❌ Failed to load settings:', error);
        }
    }, []);

    // Check connection status
    const checkConnectionStatus = useCallback(async () => {
        try {
            const status = await invoke<boolean>('get_connection_status');
            setConnectionStatus(status);
        } catch (error) {
            console.error('❌ Failed to check connection status:', error);
        }
    }, []);

    // Manual refresh
    const handleRefresh = async () => {
        setIsRefreshing(true);
        try {
            await invoke('refresh_devices');
            await loadDevices();
            await checkConnectionStatus();
        } catch (error) {
            console.error('❌ Refresh failed:', error);
        } finally {
            setIsRefreshing(false);
        }
    };

    // Initial load
    useEffect(() => {
        const loadInitialData = async () => {
            setIsLoading(true);
            await loadDevices();
            await loadSettings();
            await checkConnectionStatus();
        };

        loadInitialData();
    }, [loadDevices, loadSettings, checkConnectionStatus]);

    // Auto-refresh every 3 seconds
    useEffect(() => {
        const interval = setInterval(async () => {
            await loadDevices();
            await checkConnectionStatus();
        }, 3000);

        return () => clearInterval(interval);
    }, [loadDevices, checkConnectionStatus]);

    // Close window on Escape key
    useEffect(() => {
        const handleKeyDown = (event: KeyboardEvent) => {
            if (event.key === 'Escape') {
                invoke('hide_window');
            }
        };

        document.addEventListener('keydown', handleKeyDown);
        return () => {
            document.removeEventListener('keydown', handleKeyDown);
        };
    }, []);

    const handlePairDevice = async (deviceId: string) => {
        try {
            await invoke('pair_device', { deviceId });
            await loadDevices(); // Refresh immediately
            console.log(`✅ Paired device: ${deviceId}`);
        } catch (error) {
            console.error('❌ Failed to pair device:', error);
        }
    };

    const handleUnpairDevice = async (deviceId: string) => {
        try {
            await invoke('unpair_device', { deviceId });
            await loadDevices(); // Refresh immediately
            console.log(`✅ Unpaired device: ${deviceId}`);
        } catch (error) {
            console.error('❌ Failed to unpair device:', error);
        }
    };

    const handleRenameDevice = async (deviceId: string) => {
        if (!newDeviceName.trim()) return;

        try {
            await invoke('rename_device', { deviceId, newName: newDeviceName });
            setEditingDevice(null);
            setNewDeviceName('');
            await loadDevices();
            console.log(`✅ Renamed device: ${deviceId} -> ${newDeviceName}`);
        } catch (error) {
            console.error('❌ Failed to rename device:', error);
        }
    };

    const getDeviceIcon = (deviceType: string, isConnected: boolean) => {
        const iconClass = `w-6 h-6 ${isConnected ? 'text-green-500' : 'text-gray-400'}`;

        switch (deviceType.toLowerCase()) {
            case 'phone':
            case 'mobile':
                return <Smartphone className={iconClass} />;
            case 'laptop':
                return <Laptop className={iconClass} />;
            case 'desktop':
            default:
                return <Monitor className={iconClass} />;
        }
    };

    const getTimeAgo = (timestamp: number) => {
        const now = Math.floor(Date.now() / 1000);
        const diff = now - timestamp;

        if (diff < 60) return 'Just now';
        if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
        if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
        return `${Math.floor(diff / 86400)}d ago`;
    };

    const DeviceCard = ({ device }: { device: DeviceInfo }) => (
        <div className={`bg-white/10 backdrop-blur-sm rounded-lg p-4 border transition-all duration-200 hover:bg-white/15 ${device.is_connected
            ? 'border-green-500/50 shadow-green-500/20'
            : device.is_paired
                ? 'border-blue-500/50'
                : 'border-white/20'
            }`}>
            <div className="flex items-center justify-between mb-3">
                <div className="flex items-center space-x-3">
                    {getDeviceIcon(device.device_type, device.is_connected)}
                    <div className="flex-1">
                        {editingDevice === device.id ? (
                            <div className="flex items-center space-x-2">
                                <input
                                    type="text"
                                    value={newDeviceName}
                                    onChange={(e) => setNewDeviceName(e.target.value)}
                                    className="bg-black/20 text-white text-sm px-2 py-1 rounded border border-white/30 focus:outline-none focus:border-blue-400 w-32"
                                    placeholder={device.name}
                                    autoFocus
                                />
                                <button
                                    onClick={() => handleRenameDevice(device.id)}
                                    className="text-green-400 hover:text-green-300"
                                >
                                    <Check className="w-4 h-4" />
                                </button>
                                <button
                                    onClick={() => {
                                        setEditingDevice(null);
                                        setNewDeviceName('');
                                    }}
                                    className="text-red-400 hover:text-red-300"
                                >
                                    <X className="w-4 h-4" />
                                </button>
                            </div>
                        ) : (
                            <div className="flex items-center space-x-2">
                                <h3 className="text-white font-medium text-sm">{device.name}</h3>
                                <button
                                    onClick={() => {
                                        setEditingDevice(device.id);
                                        setNewDeviceName(device.name);
                                    }}
                                    className="text-gray-400 hover:text-white opacity-0 group-hover:opacity-100 transition-opacity"
                                >
                                    <Edit2 className="w-3 h-3" />
                                </button>
                            </div>
                        )}
                        <div className="flex items-center space-x-2 mt-1">
                            <span className="text-xs text-gray-400">{device.address}</span>
                            <span className="text-xs text-gray-500">•</span>
                            <span className="text-xs text-gray-400">{getTimeAgo(device.last_seen)}</span>
                        </div>
                    </div>
                </div>
                <div className="flex items-center space-x-2">
                    {device.is_connected ? (
                        <div className="flex items-center space-x-1">
                            <CheckCircle className="w-4 h-4 text-green-500" />
                            <span className="text-xs text-green-400">Connected</span>
                        </div>
                    ) : device.is_paired ? (
                        <div className="flex items-center space-x-1">
                            <Clock className="w-4 h-4 text-blue-500" />
                            <span className="text-xs text-blue-400">Paired</span>
                        </div>
                    ) : (
                        <div className="flex items-center space-x-1">
                            <AlertCircle className="w-4 h-4 text-gray-500" />
                            <span className="text-xs text-gray-400">Available</span>
                        </div>
                    )}
                </div>
            </div>

            <div className="flex items-center justify-between">
                <div className="flex items-center space-x-2 text-xs text-gray-400">
                    <Globe className="w-3 h-3" />
                    <span>{device.version}</span>
                </div>

                {device.is_paired ? (
                    <button
                        onClick={() => handleUnpairDevice(device.id)}
                        className="text-xs px-3 py-1 bg-red-500/20 text-red-300 rounded hover:bg-red-500/30 transition-colors"
                    >
                        Unpair
                    </button>
                ) : (
                    <button
                        onClick={() => handlePairDevice(device.id)}
                        className="text-xs px-3 py-1 bg-blue-500/20 text-blue-300 rounded hover:bg-blue-500/30 transition-colors"
                    >
                        Pair
                    </button>
                )}
            </div>
        </div>
    );

    return (
        <div className="w-full h-full bg-gradient-to-br from-slate-900/95 to-slate-800/95 backdrop-blur-xl rounded-lg border border-white/10 shadow-2xl animate-fade-in">
            {/* Close button */}
            <div className="absolute top-2 right-2 z-10">
                <button
                    onClick={() => invoke('hide_window')}
                    className="text-gray-400 hover:text-white p-1 rounded hover:bg-white/10 transition-colors"
                >
                    <X className="w-4 h-4" />
                </button>
            </div>

            {/* Header */}
            <div className="p-4 border-b border-white/10">
                <div className="flex items-center justify-between">
                    <div className="flex items-center space-x-2">
                        <div className="relative">
                            <Wifi className={`w-5 h-5 ${connectionStatus ? 'text-green-400' : 'text-gray-400'}`} />
                            {connectionStatus && (
                                <div className="absolute -top-1 -right-1 w-2 h-2 bg-green-400 rounded-full animate-pulse"></div>
                            )}
                        </div>
                        <h1 className="text-white font-semibold">Fileshare</h1>
                        {isLoading && (
                            <div className="w-3 h-3 border border-blue-400 border-t-transparent rounded-full animate-spin"></div>
                        )}
                    </div>
                    <div className="flex items-center space-x-3">
                        <button
                            onClick={handleRefresh}
                            disabled={isRefreshing}
                            className="text-gray-400 hover:text-white p-1 rounded hover:bg-white/10 transition-colors"
                        >
                            <RefreshCw className={`w-4 h-4 ${isRefreshing ? 'animate-spin' : ''}`} />
                        </button>
                        {settings && (
                            <span className="text-xs text-gray-400">{settings.device_name}</span>
                        )}
                    </div>
                </div>
                <div className="mt-2 text-xs text-gray-500">
                    Last updated: {lastUpdate.toLocaleTimeString()}
                </div>
            </div>

            {/* Tab Navigation */}
            <div className="flex border-b border-white/10">
                {[
                    { id: 'devices', label: 'Devices', icon: Monitor, count: devices.length },
                    { id: 'settings', label: 'Settings', icon: Settings },
                    { id: 'info', label: 'Info', icon: Info },
                ].map(({ id, label, icon: Icon, count }) => (
                    <button
                        key={id}
                        onClick={() => setActiveTab(id as typeof activeTab)}
                        className={`flex-1 px-4 py-3 text-sm font-medium transition-colors flex items-center justify-center space-x-2 ${activeTab === id
                            ? 'text-blue-400 border-b-2 border-blue-400 bg-blue-500/10'
                            : 'text-gray-400 hover:text-white hover:bg-white/5'
                            }`}
                    >
                        <Icon className="w-4 h-4" />
                        <span>{label}</span>
                        {count !== undefined && (
                            <span className={`px-1.5 py-0.5 text-xs rounded-full ${activeTab === id ? 'bg-blue-500/20 text-blue-300' : 'bg-gray-500/20 text-gray-400'
                                }`}>
                                {count}
                            </span>
                        )}
                    </button>
                ))}
            </div>

            {/* Content */}
            <div className="p-4 max-h-96 overflow-y-auto">
                {activeTab === 'devices' && (
                    <div className="space-y-3">
                        {isLoading ? (
                            <div className="text-center py-8">
                                <div className="w-8 h-8 border-2 border-blue-400 border-t-transparent rounded-full animate-spin mx-auto mb-3"></div>
                                <p className="text-gray-400 text-sm">Loading devices...</p>
                            </div>
                        ) : devices.length === 0 ? (
                            <div className="text-center py-8">
                                <WifiOff className="w-12 h-12 text-gray-400 mx-auto mb-3" />
                                <p className="text-gray-400 text-sm">No devices discovered</p>
                                <p className="text-gray-500 text-xs mt-1">Make sure other devices are running Fileshare</p>
                                <button
                                    onClick={handleRefresh}
                                    className="mt-3 px-3 py-1 text-xs bg-blue-500/20 text-blue-300 rounded hover:bg-blue-500/30 transition-colors"
                                >
                                    Refresh
                                </button>
                            </div>
                        ) : (
                            <>
                                <div className="flex items-center justify-between mb-3">
                                    <span className="text-xs text-gray-400">
                                        {devices.filter(d => d.is_connected).length} connected, {devices.filter(d => d.is_paired).length} paired
                                    </span>
                                    <span className="text-xs text-gray-500">
                                        Auto-refresh: 3s
                                    </span>
                                </div>
                                {devices.map((device) => (
                                    <DeviceCard key={device.id} device={device} />
                                ))}
                            </>
                        )}
                    </div>
                )}

                {activeTab === 'settings' && settings && (
                    <div className="space-y-4 animate-slide-up">
                        <div className="space-y-3">
                            <div>
                                <label className="text-sm text-gray-300 block mb-1">Device Name</label>
                                <div className="text-white bg-black/20 px-3 py-2 rounded border border-white/10">
                                    {settings.device_name}
                                </div>
                            </div>
                            <div>
                                <label className="text-sm text-gray-300 block mb-1">Device ID</label>
                                <div className="text-white bg-black/20 px-3 py-2 rounded border border-white/10 font-mono text-xs">
                                    {settings.device_id.substring(0, 8)}...
                                </div>
                            </div>
                            <div className="grid grid-cols-2 gap-3">
                                <div>
                                    <label className="text-sm text-gray-300 block mb-1">Network Port</label>
                                    <div className="text-white bg-black/20 px-3 py-2 rounded border border-white/10">
                                        {settings.network_port}
                                    </div>
                                </div>
                                <div>
                                    <label className="text-sm text-gray-300 block mb-1">Discovery Port</label>
                                    <div className="text-white bg-black/20 px-3 py-2 rounded border border-white/10">
                                        {settings.discovery_port}
                                    </div>
                                </div>
                            </div>
                            <div>
                                <label className="text-sm text-gray-300 block mb-1">Chunk Size</label>
                                <div className="text-white bg-black/20 px-3 py-2 rounded border border-white/10">
                                    {(settings.chunk_size / 1024).toFixed(0)} KB
                                </div>
                            </div>
                            <div className="space-y-2">
                                <div className="flex items-center justify-between">
                                    <span className="text-sm text-gray-300">Require Pairing</span>
                                    <div className={`w-5 h-5 rounded ${settings.require_pairing ? 'bg-green-500' : 'bg-gray-500'} flex items-center justify-center`}>
                                        {settings.require_pairing && <Check className="w-3 h-3 text-white" />}
                                    </div>
                                </div>
                                <div className="flex items-center justify-between">
                                    <span className="text-sm text-gray-300">Encryption Enabled</span>
                                    <div className={`w-5 h-5 rounded ${settings.encryption_enabled ? 'bg-green-500' : 'bg-gray-500'} flex items-center justify-center`}>
                                        {settings.encryption_enabled && <Check className="w-3 h-3 text-white" />}
                                    </div>
                                </div>
                            </div>
                        </div>
                    </div>
                )}

                {activeTab === 'info' && (
                    <div className="space-y-4 animate-slide-up">
                        <div className="bg-blue-500/10 border border-blue-500/20 rounded-lg p-4">
                            <h3 className="text-blue-300 font-medium mb-2">Hotkeys</h3>
                            <div className="space-y-2 text-sm">
                                <div className="flex justify-between text-gray-300">
                                    <span>Copy file:</span>
                                    <kbd className="bg-black/30 px-2 py-1 rounded text-xs">
                                        {/* Platform-specific hotkey display */}
                                        {navigator.platform.toLowerCase().includes('mac')
                                            ? '⌘⇧Y'
                                            : navigator.platform.toLowerCase().includes('win')
                                                ? 'Ctrl+Alt+Y'
                                                : 'Ctrl+Shift+Y'
                                        }
                                    </kbd>
                                </div>
                                <div className="flex justify-between text-gray-300">
                                    <span>Paste file:</span>
                                    <kbd className="bg-black/30 px-2 py-1 rounded text-xs">
                                        {navigator.platform.toLowerCase().includes('mac')
                                            ? '⌘⇧I'
                                            : navigator.platform.toLowerCase().includes('win')
                                                ? 'Ctrl+Alt+I'
                                                : 'Ctrl+Shift+I'
                                        }
                                    </kbd>
                                </div>
                            </div>
                        </div>

                        <div className="bg-green-500/10 border border-green-500/20 rounded-lg p-4">
                            <h3 className="text-green-300 font-medium mb-2">Status</h3>
                            <div className="space-y-2 text-sm">
                                <div className="flex justify-between text-gray-300">
                                    <span>Connected devices:</span>
                                    <span className="text-green-400">{devices.filter(d => d.is_connected).length}</span>
                                </div>
                                <div className="flex justify-between text-gray-300">
                                    <span>Paired devices:</span>
                                    <span className="text-blue-400">{devices.filter(d => d.is_paired).length}</span>
                                </div>
                                <div className="flex justify-between text-gray-300">
                                    <span>Network status:</span>
                                    <span className={connectionStatus ? 'text-green-400' : 'text-gray-400'}>
                                        {connectionStatus ? 'Online' : 'Offline'}
                                    </span>
                                </div>
                            </div>
                        </div>

                        <div className="text-center text-gray-400 text-xs">
                            <p>Fileshare v0.1.0</p>
                            <p className="mt-1">Network file sharing made simple</p>
                        </div>
                    </div>
                )}
            </div>

            {/* Footer */}
            <div className="p-4 border-t border-white/10">
                <button
                    onClick={() => invoke('quit_app')}
                    className="w-full flex items-center justify-center space-x-2 text-gray-400 hover:text-red-400 transition-colors py-2"
                >
                    <Power className="w-4 h-4" />
                    <span className="text-sm">Quit Fileshare</span>
                </button>
            </div>
        </div>
    );
}

export default App;