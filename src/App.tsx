import { useState, useEffect, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { motion, AnimatePresence } from 'framer-motion';
import { Power } from 'lucide-react';

// Import new components
import Header from './components/Headers';
import Navigation from './components/Navigation';
import DevicesList from './components/DevicesList';
import AdvancedSettings from './components/AdvancedSettings';
import EnhancedInfo from './components/EnhancedInfo';
import TransferProgress from './components/TransferProgress';
import { FadeIn, SlideIn } from './components/AnimatedComponents';
import { LoadingOverlay } from './components/LoadingStates';
import { useToast } from './hooks/useToast';
import { useDebounce } from './hooks/useDebounce';

// Types
interface DeviceInfo {
    id: string;
    name: string;
    display_name?: string;
    device_type: string;
    is_paired: boolean;
    is_connected: boolean;
    is_blocked: boolean;
    trust_level: string;
    last_seen: number;
    first_seen: number;
    connection_count: number;
    address: string;
    version: string;
    platform?: string;
    last_transfer_time?: number;
    total_transfers: number;
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
    auto_accept_from_trusted: boolean;
    block_unknown_devices: boolean;
}

function App() {
    // State
    const [devices, setDevices] = useState<DeviceInfo[]>([]);
    const [settings, setSettings] = useState<AppSettings | null>(null);
    const [activeTab, setActiveTab] = useState<'devices' | 'settings' | 'info'>('devices');
    const [isLoading, setIsLoading] = useState(true);
    const [lastUpdate, setLastUpdate] = useState<Date>(new Date());
    const [connectionStatus, setConnectionStatus] = useState(false);
    const [isRefreshing, setIsRefreshing] = useState(false);

    // Device management state
    const [selectedDevices, setSelectedDevices] = useState<Set<string>>(new Set());
    const [showBulkActions, setShowBulkActions] = useState(false);
    const [filterType, setFilterType] = useState<'all' | 'paired' | 'blocked' | 'connected'>('all');
    const [searchTerm, setSearchTerm] = useState('');
    const [sortBy, setSortBy] = useState<'name' | 'last_seen' | 'trust_level'>('name');
    const [favoriteDevices, setFavoriteDevices] = useState<Set<string>>(new Set());
    const [showTransferProgress, setShowTransferProgress] = useState(false);

    // Wrapper functions for type conversion
    const handleFilterChange = (filter: string) => {
        setFilterType(filter as 'all' | 'paired' | 'blocked' | 'connected');
    };

    const handleSortChange = (sort: string) => {
        setSortBy(sort as 'name' | 'last_seen' | 'trust_level');
    };

    // Hooks
    const debouncedSearchTerm = useDebounce(searchTerm, 300);
    const { addToast } = useToast();

    // Load data functions
    const loadDevices = useCallback(async () => {
        try {
            const discoveredDevices = await invoke<DeviceInfo[]>('get_discovered_devices');
            setDevices(discoveredDevices);
            setLastUpdate(new Date());
        } catch (error) {
            console.error('Failed to load devices:', error);
            addToast('error', 'Loading Failed', 'Failed to load devices');
        } finally {
            setIsLoading(false);
        }
    }, [addToast]);

    const loadSettings = useCallback(async () => {
        try {
            const appSettings = await invoke<AppSettings>('get_app_settings');
            setSettings(appSettings);
        } catch (error) {
            console.error('Failed to load settings:', error);
            addToast('error', 'Settings Failed', 'Failed to load settings');
        }
    }, [addToast]);

    const checkConnectionStatus = useCallback(async () => {
        try {
            const status = await invoke<boolean>('get_connection_status');
            setConnectionStatus(status);
        } catch (error) {
            console.error('Failed to check connection status:', error);
        }
    }, []);

    // Device filtering and sorting
    const getFilteredDevices = () => {
        let filtered = devices;

        if (debouncedSearchTerm) {
            filtered = filtered.filter(device =>
                device.name.toLowerCase().includes(debouncedSearchTerm.toLowerCase()) ||
                (device.display_name && device.display_name.toLowerCase().includes(debouncedSearchTerm.toLowerCase())) ||
                device.address.includes(debouncedSearchTerm) ||
                (device.platform && device.platform.toLowerCase().includes(debouncedSearchTerm.toLowerCase()))
            );
        }

        switch (filterType) {
            case 'paired':
                filtered = filtered.filter(d => d.is_paired);
                break;
            case 'blocked':
                filtered = filtered.filter(d => d.is_blocked);
                break;
            case 'connected':
                filtered = filtered.filter(d => d.is_connected);
                break;
        }

        filtered.sort((a, b) => {
            const aFav = favoriteDevices.has(a.id);
            const bFav = favoriteDevices.has(b.id);
            if (aFav && !bFav) return -1;
            if (!aFav && bFav) return 1;

            switch (sortBy) {
                case 'name':
                    return (a.display_name || a.name).localeCompare(b.display_name || b.name);
                case 'last_seen':
                    return b.last_seen - a.last_seen;
                case 'trust_level':
                    const trustOrder = { 'Verified': 3, 'Trusted': 2, 'Unknown': 1, 'Blocked': 0 };
                    return trustOrder[b.trust_level as keyof typeof trustOrder] - trustOrder[a.trust_level as keyof typeof trustOrder];
                default:
                    return 0;
            }
        });

        return filtered;
    };

    // Device actions
    const deviceActions = {
        onPair: async (deviceId: string) => {
            try {
                await invoke('pair_device', { deviceId });
                await loadDevices();
                addToast('success', 'Device Paired', 'Device paired successfully');
            } catch (error) {
                addToast('error', 'Pairing Failed', `Failed to pair device: ${error}`);
            }
        },
        onUnpair: async (deviceId: string) => {
            try {
                await invoke('unpair_device', { deviceId });
                await loadDevices();
                addToast('info', 'Device Unpaired', 'Device unpaired successfully');
            } catch (error) {
                addToast('error', 'Unpair Failed', `Failed to unpair device: ${error}`);
            }
        },
        onBlock: async (deviceId: string) => {
            try {
                await invoke('block_device', { deviceId });
                await loadDevices();
                addToast('warning', 'Device Blocked', 'Device blocked successfully');
            } catch (error) {
                addToast('error', 'Block Failed', `Failed to block device: ${error}`);
            }
        },
        onUnblock: async (deviceId: string) => {
            try {
                await invoke('unblock_device', { deviceId });
                await loadDevices();
                addToast('success', 'Device Unblocked', 'Device unblocked successfully');
            } catch (error) {
                addToast('error', 'Unblock Failed', `Failed to unblock device: ${error}`);
            }
        },
        onForget: async (deviceId: string) => {
            try {
                await invoke('forget_device', { deviceId });
                await loadDevices();
                addToast('info', 'Device Forgotten', 'Device forgotten successfully');
            } catch (error) {
                addToast('error', 'Forget Failed', `Failed to forget device: ${error}`);
            }
        },
        onRename: async (deviceId: string, newName: string) => {
            try {
                await invoke('rename_device_enhanced', { deviceId, newName });
                await loadDevices();
                addToast('success', 'Device Renamed', `Device renamed to "${newName}"`);
            } catch (error) {
                addToast('error', 'Rename Failed', `Failed to rename device: ${error}`);
            }
        },
        onToggleFavorite: (deviceId: string) => {
            const newFavorites = new Set(favoriteDevices);
            if (newFavorites.has(deviceId)) {
                newFavorites.delete(deviceId);
                addToast('info', 'Removed from Favorites', 'Device removed from favorites');
            } else {
                newFavorites.add(deviceId);
                addToast('success', 'Added to Favorites', 'Device added to favorites');
            }
            setFavoriteDevices(newFavorites);
            localStorage.setItem('favoriteDevices', JSON.stringify(Array.from(newFavorites)));
        },
        onConnect: async (deviceId: string) => {
            try {
                await invoke('connect_to_peer', { deviceId });
                await loadDevices();
                addToast('success', 'Connected', 'Connected to device');
            } catch (error) {
                addToast('error', 'Connection Failed', `Failed to connect: ${error}`);
            }
        },
        onDisconnect: async (deviceId: string) => {
            try {
                await invoke('disconnect_from_peer', { deviceId });
                await loadDevices();
                addToast('info', 'Disconnected', 'Disconnected from device');
            } catch (error) {
                addToast('error', 'Disconnect Failed', `Failed to disconnect: ${error}`);
            }
        }
    };

    // Other handlers
    const handleRefresh = async () => {
        setIsRefreshing(true);
        try {
            await invoke('refresh_devices');
            await loadDevices();
            await checkConnectionStatus();
            addToast('success', 'Refreshed', 'Device list updated');
        } catch (error) {
            addToast('error', 'Refresh Failed', 'Failed to refresh devices');
        } finally {
            setIsRefreshing(false);
        }
    };

    const handleDeviceSelect = (deviceId: string) => {
        const newSelected = new Set(selectedDevices);
        if (newSelected.has(deviceId)) {
            newSelected.delete(deviceId);
        } else {
            newSelected.add(deviceId);
        }
        setSelectedDevices(newSelected);
        setShowBulkActions(newSelected.size > 0);
    };

    const handleSelectAll = () => {
        const filteredDeviceIds = getFilteredDevices().map(d => d.id);
        setSelectedDevices(new Set(filteredDeviceIds));
        setShowBulkActions(filteredDeviceIds.length > 0);
    };

    const handleClearSelection = () => {
        setSelectedDevices(new Set());
        setShowBulkActions(false);
    };

    const handleBulkAction = async (action: string) => {
        if (selectedDevices.size === 0) return;
        try {
            const deviceIds = Array.from(selectedDevices);
            const successCount = await invoke<number>('bulk_device_action', { action, deviceIds });
            setSelectedDevices(new Set());
            setShowBulkActions(false);
            await loadDevices();
            addToast('success', 'Bulk Operation Complete', `${action} completed on ${successCount} devices`);
        } catch (error) {
            addToast('error', 'Bulk Operation Failed', `Failed to ${action} devices: ${error}`);
        }
    };

    // Load initial data
    useEffect(() => {
        const loadInitialData = async () => {
            setIsLoading(true);
            await loadDevices();
            await loadSettings();
            await checkConnectionStatus();
        };
        loadInitialData();
    }, [loadDevices, loadSettings, checkConnectionStatus]);

    // Auto-refresh
    useEffect(() => {
        const interval = setInterval(async () => {
            await loadDevices();
            await checkConnectionStatus();
        }, 3000);
        return () => clearInterval(interval);
    }, [loadDevices, checkConnectionStatus]);

    // Load favorites
    useEffect(() => {
        const saved = localStorage.getItem('favoriteDevices');
        if (saved) {
            try {
                const favorites = JSON.parse(saved);
                setFavoriteDevices(new Set(favorites));
            } catch (error) {
                console.error('Failed to load favorite devices:', error);
            }
        }
    }, []);

    const filteredDevices = getFilteredDevices();

    return (
        <div className="app-container w-full h-full bg-gradient-to-br from-slate-900/95 to-slate-800/95 backdrop-blur-xl border border-white/10 shadow-2xl relative">
            <LoadingOverlay isVisible={isLoading && devices.length === 0} message="Loading devices..." />

            {/* Header */}
            <FadeIn>
                <Header
                    connectionStatus={connectionStatus}
                    isLoading={isLoading}
                    isRefreshing={isRefreshing}
                    lastUpdate={lastUpdate}
                    filteredDevicesCount={filteredDevices.length}
                    deviceName={settings?.device_name}
                    onRefresh={handleRefresh}
                    onSelectAll={handleSelectAll}
                />
            </FadeIn>

            {/* Navigation */}
            <SlideIn direction="up">
                <Navigation
                    activeTab={activeTab}
                    onTabChange={setActiveTab}
                    deviceCount={filteredDevices.length}
                />
            </SlideIn>

            {/* Content */}
            <div className="p-4 min-h-[475px] overflow-y-auto">
                <AnimatePresence mode="wait">
                    {activeTab === 'devices' && (
                        <FadeIn key="devices">
                            <DevicesList
                                devices={devices}
                                filteredDevices={filteredDevices}
                                isLoading={isLoading}
                                searchTerm={searchTerm}
                                filterType={filterType}
                                selectedDevices={selectedDevices}
                                showBulkActions={showBulkActions}
                                favoriteDevices={favoriteDevices}
                                onSearchChange={setSearchTerm}
                                onFilterChange={handleFilterChange}
                                onSortChange={handleSortChange}
                                onDeviceSelect={handleDeviceSelect}
                                onSelectAll={handleSelectAll}
                                onClearSelection={handleClearSelection}
                                onBulkAction={handleBulkAction}
                                onRefresh={handleRefresh}
                                deviceActions={deviceActions}
                            />
                        </FadeIn>
                    )}

                    {activeTab === 'settings' && settings && (
                        <FadeIn key="settings">
                            <AdvancedSettings
                                settings={settings}
                                onSettingsChange={(newSettings) => {
                                    setSettings(newSettings);
                                    loadDevices();
                                }}
                            />
                        </FadeIn>
                    )}

                    {activeTab === 'info' && (
                        <FadeIn key="info">
                            <EnhancedInfo />
                        </FadeIn>
                    )}
                </AnimatePresence>
            </div>

            {/* Transfer Progress */}
            <AnimatePresence>
                {showTransferProgress && (
                    <TransferProgress
                        isVisible={showTransferProgress}
                        onToggle={() => setShowTransferProgress(false)}
                    />
                )}
            </AnimatePresence>

            {/* Footer */}
            <SlideIn direction="up">
                <div className="p-4 border-t border-white/10">
                    <motion.button
                        whileHover={{ scale: 1.02 }}
                        whileTap={{ scale: 0.98 }}
                        onClick={() => invoke('quit_app')}
                        className="w-full flex items-center justify-center space-x-2 text-gray-400 hover:text-red-400 transition-colors py-2"
                    >
                        <Power className="w-4 h-4" />
                        <span className="text-sm">Quit Fileshare</span>
                    </motion.button>
                </div>
            </SlideIn>
        </div>
    );
}

export default App;