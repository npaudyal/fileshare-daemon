import { useState, useEffect, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { motion, AnimatePresence } from 'framer-motion';
import { Power } from 'lucide-react';

// Import new components
import Header from './components/Headers';
import Navigation from './components/Navigation';
import DevicesList from './components/DevicesList';
import PairingTab from './components/PairingTab';
import AdvancedSettings from './components/AdvancedSettings';
import EnhancedInfo from './components/EnhancedInfo';
import TransferProgress from './components/TransferProgress';
import TransferProgressView from './components/TransferProgressView';
import ViewTransition from './components/ViewTransition';
import ThemeToggle from './components/ThemeToggle';
import { FadeIn, SlideIn } from './components/AnimatedComponents';
import { LoadingOverlay } from './components/LoadingStates';
import { useToast } from './hooks/useToast';
import { useDebounce } from './hooks/useDebounce';
import { useTransferView } from './hooks/useTransferView';
import { useViewStore } from './stores/viewStore';
import { useTheme } from './context/ThemeContext';

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
    const { theme } = useTheme();
    const { windowMode, isTransitioning } = useViewStore();
    const { hasActiveTransfers } = useTransferView();

    // State
    const [devices, setDevices] = useState<DeviceInfo[]>([]);
    const [settings, setSettings] = useState<AppSettings | null>(null);
    const [activeTab, setActiveTab] = useState<'devices' | 'pairing' | 'settings' | 'info'>('devices');
    const [isLoading, setIsLoading] = useState(true);
    const [lastUpdate, setLastUpdate] = useState<Date>(new Date());
    const [connectionStatus, setConnectionStatus] = useState(false);
    const [isRefreshing, setIsRefreshing] = useState(false);

    // Device management state
    const [selectedDevices, setSelectedDevices] = useState<Set<string>>(new Set());
    const [showBulkActions, setShowBulkActions] = useState(false);
    const [searchTerm, setSearchTerm] = useState('');
    const [sortBy, setSortBy] = useState<'name' | 'last_seen' | 'trust_level'>('name');
    const [favoriteDevices, setFavoriteDevices] = useState<Set<string>>(new Set());
    const [showTransferProgress, setShowTransferProgress] = useState(false);

    // Wrapper functions for type conversion

    const handleSortChange = (sort: string) => {
        setSortBy(sort as 'name' | 'last_seen' | 'trust_level');
    };

    // Hooks
    const debouncedSearchTerm = useDebounce(searchTerm, 300);
    const { addToast } = useToast();

    // Load data functions
    const loadDevices = useCallback(async () => {
        try {
            // Get all discovered devices (currently online/broadcasting)
            const discoveredDevices = await invoke<DeviceInfo[]>('get_discovered_devices');
            const discoveredMap = new Map(discoveredDevices.map(d => [d.id, d]));

            // Get paired devices from storage (persistent)
            const pairedDevicesData = await invoke<any[]>('get_paired_devices');

            // Create a merged list: all paired devices + discovered unpaired devices
            const mergedDevices: DeviceInfo[] = [];

            // First, add all paired devices with their online/offline status
            for (const pairedDevice of pairedDevicesData) {
                const discovered = discoveredMap.get(pairedDevice.device_id);

                if (discovered) {
                    // Device is paired AND currently discovered (online)
                    mergedDevices.push({
                        ...discovered,
                        is_paired: true,
                        is_connected: true,
                        display_name: pairedDevice.name || discovered.display_name,
                        trust_level: pairedDevice.trust_level || discovered.trust_level,
                        last_seen: discovered.last_seen // Use current last seen time
                    });
                } else {
                    // Device is paired but NOT currently discovered (offline)
                    mergedDevices.push({
                        id: pairedDevice.device_id,
                        name: pairedDevice.name,
                        display_name: pairedDevice.name,
                        device_type: 'desktop', // Default, will be updated when device comes online
                        is_paired: true,
                        is_connected: false,
                        is_blocked: false,
                        trust_level: pairedDevice.trust_level || 'Trusted',
                        last_seen: pairedDevice.last_seen || 0,
                        first_seen: pairedDevice.paired_at || 0,
                        connection_count: 0,
                        address: 'Unknown',
                        version: 'Unknown',
                        platform: undefined,
                        last_transfer_time: undefined,
                        total_transfers: 0
                    });
                }
            }

            // Then, add discovered devices that are not paired
            const pairedIds = new Set(pairedDevicesData.map(d => d.device_id));
            for (const discovered of discoveredDevices) {
                if (!pairedIds.has(discovered.id)) {
                    mergedDevices.push({
                        ...discovered,
                        is_paired: false,
                        is_connected: true // If discovered, it's online
                    });
                }
            }

            setDevices(mergedDevices);
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

    // Get paired devices for DEVICES tab
    const getPairedDevices = () => {
        let filtered = devices.filter(d => d.is_paired);

        if (debouncedSearchTerm) {
            filtered = filtered.filter(device =>
                device.name.toLowerCase().includes(debouncedSearchTerm.toLowerCase()) ||
                (device.display_name && device.display_name.toLowerCase().includes(debouncedSearchTerm.toLowerCase())) ||
                device.address.includes(debouncedSearchTerm) ||
                (device.platform && device.platform.toLowerCase().includes(debouncedSearchTerm.toLowerCase()))
            );
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

    // Get unpaired devices for PAIRING tab
    const getUnpairedDevices = () => {
        return devices.filter(d => !d.is_paired);
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
        const filteredDeviceIds = pairedDevices.map((d: any) => d.device_id);
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

    // Auto-refresh with faster updates during pairing
    useEffect(() => {
        // Faster refresh when in pairing tab
        const refreshInterval = activeTab === 'pairing' ? 1000 : 3000;

        const interval = setInterval(async () => {
            await loadDevices();
            await checkConnectionStatus();
        }, refreshInterval);
        return () => clearInterval(interval);
    }, [loadDevices, checkConnectionStatus, activeTab]);

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

    // Get device lists
    const pairedDevices = getPairedDevices();
    const unpairedDevices = getUnpairedDevices();

    return (
        <div
            className="app-container w-full h-full backdrop-blur-xl shadow-2xl relative transition-all duration-300"
            style={{
                background: theme.gradients.primary,
                borderColor: theme.colors.border,
                border: '1px solid',
            }}
        >
            {/* Transfer View */}
            <ViewTransition mode="Transfer">
                <TransferProgressView />
            </ViewTransition>

            {/* Normal View */}
            <ViewTransition mode="Normal">
                <div className="w-full h-full flex flex-col">
                    <LoadingOverlay isVisible={isLoading && devices.length === 0} message="Loading devices..." />

                    {/* Header */}
                    <FadeIn>
                        <Header deviceName={settings?.device_name} />
                    </FadeIn>

                    {/* Navigation */}
                    <SlideIn direction="up">
                        <Navigation
                            activeTab={activeTab}
                            onTabChange={setActiveTab}
                            deviceCount={pairedDevices.length}
                            unpairedDeviceCount={unpairedDevices.length}
                        />
                    </SlideIn>

                    {/* Content */}
                    <div className="p-4 min-h-[375px] overflow-y-auto">
                <AnimatePresence mode="wait">
                    {activeTab === 'devices' && (
                        <FadeIn key="devices">
                            <DevicesList
                                devices={devices}
                                filteredDevices={pairedDevices}
                                isLoading={isLoading}
                                searchTerm={searchTerm}
                                selectedDevices={selectedDevices}
                                showBulkActions={showBulkActions}
                                favoriteDevices={favoriteDevices}
                                onSearchChange={setSearchTerm}
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

                    {activeTab === 'pairing' && (
                        <FadeIn key="pairing">
                            <PairingTab
                                onRefresh={handleRefresh}
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
                        <div
                            className="p-4 border-t transition-all duration-300"
                            style={{ borderColor: theme.colors.border }}
                        >
                    <div className="flex items-center justify-between">
                        {/* Theme Switcher - Left Side */}
                        <ThemeToggle />

                        {/* Quit Button - Right Side */}
                        <motion.button
                            whileHover={{ scale: 1.02 }}
                            whileTap={{ scale: 0.98 }}
                            onClick={() => invoke('quit_app')}
                            className="flex items-center space-x-2 px-4 py-2 rounded-lg transition-all duration-300"
                            style={{
                                color: theme.colors.textSecondary,
                                backgroundColor: theme.colors.backgroundSecondary,
                                border: `1px solid ${theme.colors.border}`,
                            }}
                            onMouseEnter={(e) => {
                                e.currentTarget.style.color = theme.colors.error;
                                e.currentTarget.style.borderColor = theme.colors.error;
                                e.currentTarget.style.boxShadow = `0 0 12px ${theme.colors.error}40`;
                            }}
                            onMouseLeave={(e) => {
                                e.currentTarget.style.color = theme.colors.textSecondary;
                                e.currentTarget.style.borderColor = theme.colors.border;
                                e.currentTarget.style.boxShadow = 'none';
                            }}
                        >
                            <Power className="w-4 h-4" />
                            <span className="text-sm font-medium">Quit Fileshare</span>
                        </motion.button>
                    </div>
                </div>
            </SlideIn>
                </div>
            </ViewTransition>
        </div>
    );
}

export default App;