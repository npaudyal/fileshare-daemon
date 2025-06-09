import React, { useState, useEffect } from 'react';
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
    X
} from 'lucide-react';

interface DeviceInfo {
    id: string;
    name: string;
    device_type: string;
    is_paired: boolean;
    is_connected: boolean;
    last_seen: number;
}

interface AppSettings {
    device_name: string;
    device_id: string;
    network_port: number;
    discovery_port: number;
    chunk_size: number;
    max_concurrent_transfers: number;
}

function App() {
    const [devices, setDevices] = useState<DeviceInfo[]>([]);
    const [settings, setSettings] = useState<AppSettings | null>(null);
    const [activeTab, setActiveTab] = useState<'devices' | 'settings' | 'info'>('devices');
    const [editingDevice, setEditingDevice] = useState<string | null>(null);
    const [newDeviceName, setNewDeviceName] = useState<string>('');

    useEffect(() => {
        loadDevices();
        loadSettings();

        // Auto-refresh devices every 3 seconds
        const interval = setInterval(loadDevices, 3000);
        return () => clearInterval(interval);
    }, []);

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

    const loadDevices = async () => {
        try {
            const discoveredDevices = await invoke<DeviceInfo[]>('get_discovered_devices');
            setDevices(discoveredDevices);
        } catch (error) {
            console.error('Failed to load devices:', error);
        }
    };

    const loadSettings = async () => {
        try {
            const appSettings = await invoke<AppSettings>('get_app_settings');
            setSettings(appSettings);
        } catch (error) {
            console.error('Failed to load settings:', error);
        }
    };

    const handlePairDevice = async (deviceId: string) => {
        try {
            await invoke('pair_device', { deviceId });
            loadDevices();
        } catch (error) {
            console.error('Failed to pair device:', error);
        }
    };

    const handleUnpairDevice = async (deviceId: string) => {
        try {
            await invoke('unpair_device', { deviceId });
            loadDevices();
        } catch (error) {
            console.error('Failed to unpair device:', error);
        }
    };

    const handleRenameDevice = async (deviceId: string) => {
        if (!newDeviceName.trim()) return;

        try {
            await invoke('rename_device', { deviceId, newName: newDeviceName });
            setEditingDevice(null);
            setNewDeviceName('');
            loadDevices();
        } catch (error) {
            console.error('Failed to rename device:', error);
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

    const DeviceCard = ({ device }: { device: DeviceInfo }) => (
        <div className="bg-white/10 backdrop-blur-sm rounded-lg p-4 border border-white/20 hover:bg-white/15 transition-all duration-200">
            <div className="flex items-center justify-between mb-2">
                <div className="flex items-center space-x-3">
                    {getDeviceIcon(device.device_type, device.is_connected)}
                    <div>
                        {editingDevice === device.id ? (
                            <div className="flex items-center space-x-2">
                                <input
                                    type="text"
                                    value={newDeviceName}
                                    onChange={(e) => setNewDeviceName(e.target.value)}
                                    className="bg-black/20 text-white text-sm px-2 py-1 rounded border border-white/30 focus:outline-none focus:border-blue-400"
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
                                    className="text-gray-400 hover:text-white"
                                >
                                    <Edit2 className="w-3 h-3" />
                                </button>
                            </div>
                        )}
                    </div>
                </div>
                <div className="flex items-center space-x-2">
                    {device.is_connected ? (
                        <Wifi className="w-4 h-4 text-green-500" />
                    ) : (
                        <WifiOff className="w-4 h-4 text-gray-400" />
                    )}
                </div>
            </div>

            <div className="flex items-center justify-between">
                <span className={`text-xs px-2 py-1 rounded-full ${device.is_paired
                    ? 'bg-green-500/20 text-green-300'
                    : 'bg-gray-500/20 text-gray-300'
                    }`}>
                    {device.is_paired ? 'Paired' : 'Available'}
                </span>

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
        <div
            className="w-full h-full bg-gradient-to-br from-slate-900/95 to-slate-800/95 backdrop-blur-xl rounded-lg border border-white/10 shadow-2xl animate-fade-in"
            onMouseLeave={() => {
                // Optional: Auto-hide when mouse leaves the window area
                // invoke('hide_window');
            }}
        >
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
                    <h1 className="text-white font-semibold flex items-center space-x-2">
                        <Wifi className="w-5 h-5 text-blue-400" />
                        <span>Fileshare</span>
                    </h1>
                    {settings && (
                        <span className="text-xs text-gray-400">{settings.device_name}</span>
                    )}
                </div>
            </div>

            {/* Tab Navigation */}
            <div className="flex border-b border-white/10">
                {[
                    { id: 'devices', label: 'Devices', icon: Monitor },
                    { id: 'settings', label: 'Settings', icon: Settings },
                    { id: 'info', label: 'Info', icon: Info },
                ].map(({ id, label, icon: Icon }) => (
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
                    </button>
                ))}
            </div>

            {/* Content */}
            <div className="p-4 max-h-80 overflow-y-auto">
                {activeTab === 'devices' && (
                    <div className="space-y-3">
                        {devices.length === 0 ? (
                            <div className="text-center py-8">
                                <WifiOff className="w-12 h-12 text-gray-400 mx-auto mb-3" />
                                <p className="text-gray-400 text-sm">No devices discovered</p>
                                <p className="text-gray-500 text-xs mt-1">Searching for nearby devices...</p>
                            </div>
                        ) : (
                            devices.map((device) => (
                                <DeviceCard key={device.id} device={device} />
                            ))
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
                            <div>
                                <label className="text-sm text-gray-300 block mb-1">Chunk Size</label>
                                <div className="text-white bg-black/20 px-3 py-2 rounded border border-white/10">
                                    {(settings.chunk_size / 1024).toFixed(0)} KB
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
                                    <kbd className="bg-black/30 px-2 py-1 rounded text-xs">⌘⇧Y</kbd>
                                </div>
                                <div className="flex justify-between text-gray-300">
                                    <span>Paste file:</span>
                                    <kbd className="bg-black/30 px-2 py-1 rounded text-xs">⌘⇧I</kbd>
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