import React, { useState, useEffect, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { motion, AnimatePresence } from 'framer-motion';
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
    Globe,
    MoreVertical,
    Shield,
    ShieldCheck,
    ShieldX,
    Trash2,
    UserX,
    Eye,
    Filter,
    Search,
    ChevronDown,
    Tablet,
    Server,
    Star,
    AlertTriangle,
    Ban,
    CheckSquare,
    Square,
    Download,
    Upload
} from 'lucide-react';

// Import enhanced components
import { FadeIn, SlideIn, StaggeredList, FloatingButton } from './components/AnimatedComponents';
import TransferProgress from './components/TransferProgress';
import { DeviceCardSkeleton, LoadingOverlay } from './components/LoadingStates';
import { useClickOutside } from './hooks/useClickOutside';
import { useDebounce } from './hooks/useDebounce';
import { useToast } from './hooks/useToast';
import AdvancedSettings from './components/AdvancedSettings';
import EnhancedInfo from './components/EnhancedInfo';
import DeviceManagement from './components/DeviceManagement';

// Enhanced interfaces
interface DeviceInfo {
    id: string;
    name: string;
    display_name?: string;
    device_type: string;
    is_paired: boolean;
    is_connected: boolean;
    is_blocked: boolean;
    trust_level: TrustLevel;
    last_seen: number;
    first_seen: number;
    connection_count: number;
    address: string;
    version: string;
    platform?: string;
    last_transfer_time?: number;
    total_transfers: number;
}

type TrustLevel = 'Unknown' | 'Trusted' | 'Verified' | 'Blocked';

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

interface ConfirmDialog {
    isOpen: boolean;
    title: string;
    message: string;
    confirmText: string;
    confirmVariant: 'danger' | 'warning' | 'primary';
    onConfirm: () => void;
    onCancel: () => void;
}

interface DeviceDetailsModal {
    isOpen: boolean;
    device: DeviceInfo | null;
}

interface DeviceStats {
    transferSpeed: number;
    successRate: number;
    lastActivity: number;
    totalData: number;
}

interface FavoriteDevice {
    deviceId: string;
    addedAt: number;
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

    // Enhanced state for device management
    const [selectedDevices, setSelectedDevices] = useState<Set<string>>(new Set());
    const [showBulkActions, setShowBulkActions] = useState(false);
    const [filterType, setFilterType] = useState<'all' | 'paired' | 'blocked' | 'connected'>('all');
    const [searchTerm, setSearchTerm] = useState('');
    const [sortBy, setSortBy] = useState<'name' | 'last_seen' | 'trust_level'>('name');
    const [confirmDialog, setConfirmDialog] = useState<ConfirmDialog>({
        isOpen: false,
        title: '',
        message: '',
        confirmText: '',
        confirmVariant: 'primary',
        onConfirm: () => { },
        onCancel: () => { }
    });
    const [deviceDetailsModal, setDeviceDetailsModal] = useState<DeviceDetailsModal>({
        isOpen: false,
        device: null
    });
    const [deviceMenuOpen, setDeviceMenuOpen] = useState<string | null>(null);

    // Phase 5 enhancements
    const [showTransferProgress, setShowTransferProgress] = useState(false);
    const [windowFocused, setWindowFocused] = useState(true);
    const [autoHideTimer, setAutoHideTimer] = useState<NodeJS.Timeout | null>(null);
    const [favoriteDevices, setFavoriteDevices] = useState<Set<string>>(new Set());
    const [deviceStats, setDeviceStats] = useState<Map<string, DeviceStats>>(new Map());

    // Performance optimizations
    const debouncedSearchTerm = useDebounce(searchTerm, 300);
    const { addToast } = useToast();

    // Click outside detection
    const appRef = useClickOutside(() => {
        if (windowFocused && !confirmDialog.isOpen && !deviceDetailsModal.isOpen) {
            invoke('hide_window');
        }
    });

    // Auto-hide functionality
    useEffect(() => {
        const handleFocus = () => setWindowFocused(true);
        const handleBlur = () => setWindowFocused(false);

        window.addEventListener('focus', handleFocus);
        window.addEventListener('blur', handleBlur);

        return () => {
            window.removeEventListener('focus', handleFocus);
            window.removeEventListener('blur', handleBlur);
        };
    }, []);

    // Auto-hide after inactivity
    useEffect(() => {
        const resetTimer = () => {
            if (autoHideTimer) clearTimeout(autoHideTimer);
            setAutoHideTimer(setTimeout(() => {
                if (windowFocused) {
                    invoke('hide_window');
                }
            }, 300000)); // 5 minutes
        };

        const events = ['mousedown', 'mousemove', 'keypress', 'scroll', 'touchstart'];
        events.forEach(event => document.addEventListener(event, resetTimer, true));

        resetTimer();

        return () => {
            if (autoHideTimer) clearTimeout(autoHideTimer);
            events.forEach(event => document.removeEventListener(event, resetTimer, true));
        };
    }, [autoHideTimer, windowFocused]);

    // Load favorites from localStorage on startup
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

    // Load devices with better error handling
    const loadDevices = useCallback(async () => {
        try {
            const discoveredDevices = await invoke<DeviceInfo[]>('get_discovered_devices');
            setDevices(discoveredDevices);
            setLastUpdate(new Date());
            console.log(`📱 Loaded ${discoveredDevices.length} devices`);
        } catch (error) {
            console.error('❌ Failed to load devices:', error);
            addToast('error', 'Loading Failed', 'Failed to load devices');
        } finally {
            setIsLoading(false);
        }
    }, [addToast]);

    // Load settings
    const loadSettings = useCallback(async () => {
        try {
            const appSettings = await invoke<AppSettings>('get_app_settings');
            setSettings(appSettings);
            console.log('⚙️ Settings loaded');
        } catch (error) {
            console.error('❌ Failed to load settings:', error);
            addToast('error', 'Settings Failed', 'Failed to load settings');
        }
    }, [addToast]);

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
            addToast('success', 'Refreshed', 'Device list updated');
        } catch (error) {
            console.error('❌ Refresh failed:', error);
            addToast('error', 'Refresh Failed', 'Failed to refresh devices');
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

    // Enhanced device actions with toast notifications
    const handlePairDevice = async (deviceId: string, trustLevel: TrustLevel = 'Trusted') => {
        try {
            await invoke('pair_device_with_trust', { deviceId, trustLevel });
            await loadDevices();

            const device = devices.find(d => d.id === deviceId);
            const deviceName = device?.display_name || device?.name || 'Unknown Device';

            addToast('success', 'Device Paired', `Successfully paired with ${deviceName}`, {
                icon: <Shield className="w-5 h-5" />
            });

            console.log(`✅ Paired device: ${deviceId} with trust: ${trustLevel}`);
        } catch (error) {
            console.error('❌ Failed to pair device:', error);
            addToast('error', 'Pairing Failed', `Failed to pair with device: ${error}`, {
                icon: <AlertCircle className="w-5 h-5" />
            });
        }
    };

    const handleUnpairDevice = async (deviceId: string) => {
        try {
            await invoke('unpair_device', { deviceId });
            await loadDevices();

            const device = devices.find(d => d.id === deviceId);
            const deviceName = device?.display_name || device?.name || 'Unknown Device';

            addToast('info', 'Device Unpaired', `${deviceName} has been unpaired`, {
                icon: <UserX className="w-5 h-5" />
            });

            console.log(`✅ Unpaired device: ${deviceId}`);
        } catch (error) {
            console.error('❌ Failed to unpair device:', error);
            addToast('error', 'Unpair Failed', `Failed to unpair device: ${error}`);
        }
    };

    const handleBlockDevice = async (deviceId: string) => {
        try {
            await invoke('block_device', { deviceId });
            await loadDevices();

            const device = devices.find(d => d.id === deviceId);
            const deviceName = device?.display_name || device?.name || 'Unknown Device';

            addToast('warning', 'Device Blocked', `${deviceName} has been blocked`, {
                icon: <Ban className="w-5 h-5" />
            });

            console.log(`🚫 Blocked device: ${deviceId}`);
        } catch (error) {
            console.error('❌ Failed to block device:', error);
            addToast('error', 'Block Failed', `Failed to block device: ${error}`);
        }
    };

    const handleUnblockDevice = async (deviceId: string) => {
        try {
            await invoke('unblock_device', { deviceId });
            await loadDevices();

            const device = devices.find(d => d.id === deviceId);
            const deviceName = device?.display_name || device?.name || 'Unknown Device';

            addToast('success', 'Device Unblocked', `${deviceName} has been unblocked`, {
                icon: <Check className="w-5 h-5" />
            });

            console.log(`✅ Unblocked device: ${deviceId}`);
        } catch (error) {
            console.error('❌ Failed to unblock device:', error);
            addToast('error', 'Unblock Failed', `Failed to unblock device: ${error}`);
        }
    };

    const handleForgetDevice = async (deviceId: string) => {
        try {
            await invoke('forget_device', { deviceId });
            await loadDevices();

            const device = devices.find(d => d.id === deviceId);
            const deviceName = device?.display_name || device?.name || 'Unknown Device';

            addToast('info', 'Device Forgotten', `${deviceName} has been forgotten`, {
                icon: <Trash2 className="w-5 h-5" />
            });

            console.log(`🗑️ Forgot device: ${deviceId}`);
        } catch (error) {
            console.error('❌ Failed to forget device:', error);
            addToast('error', 'Forget Failed', `Failed to forget device: ${error}`);
        }
    };

    const handleRenameDevice = async (deviceId: string) => {
        if (!newDeviceName.trim()) return;

        // Validate name
        if (newDeviceName.length > 50) {
            addToast('error', 'Name Too Long', 'Device name must be 50 characters or less');
            return;
        }

        if (newDeviceName.match(/[<>:"|?*\/\\]/)) {
            addToast('error', 'Invalid Characters', 'Device name contains forbidden characters');
            return;
        }

        try {
            await invoke('rename_device_enhanced', { deviceId, newName: newDeviceName });
            setEditingDevice(null);
            setNewDeviceName('');
            await loadDevices();

            addToast('success', 'Device Renamed', `Device renamed to "${newDeviceName}"`, {
                icon: <Edit2 className="w-5 h-5" />
            });

            console.log(`✅ Renamed device: ${deviceId} -> ${newDeviceName}`);
        } catch (error) {
            console.error('❌ Failed to rename device:', error);
            addToast('error', 'Rename Failed', `Failed to rename device: ${error}`);
        }
    };

    // Device connection management
    const handleConnectDevice = async (deviceId: string) => {
        try {
            await invoke('connect_to_peer', { deviceId });
            await loadDevices();

            const device = devices.find(d => d.id === deviceId);
            const deviceName = device?.display_name || device?.name || 'Unknown Device';

            addToast('success', 'Connected', `Connected to ${deviceName}`, {
                icon: <Wifi className="w-5 h-5" />
            });
        } catch (error) {
            console.error('❌ Failed to connect to device:', error);
            addToast('error', 'Connection Failed', `Failed to connect to device: ${error}`);
        }
    };

    const handleDisconnectDevice = async (deviceId: string) => {
        try {
            await invoke('disconnect_from_peer', { deviceId });
            await loadDevices();

            const device = devices.find(d => d.id === deviceId);
            const deviceName = device?.display_name || device?.name || 'Unknown Device';

            addToast('info', 'Disconnected', `Disconnected from ${deviceName}`, {
                icon: <WifiOff className="w-5 h-5" />
            });
        } catch (error) {
            console.error('❌ Failed to disconnect from device:', error);
            addToast('error', 'Disconnect Failed', `Failed to disconnect from device: ${error}`);
        }
    };

    // Favorite device management
    const handleToggleFavorite = (deviceId: string) => {
        const newFavorites = new Set(favoriteDevices);
        if (newFavorites.has(deviceId)) {
            newFavorites.delete(deviceId);
            addToast('info', 'Removed from Favorites', 'Device removed from favorites');
        } else {
            newFavorites.add(deviceId);
            addToast('success', 'Added to Favorites', 'Device added to favorites', {
                icon: <Star className="w-5 h-5" />
            });
        }
        setFavoriteDevices(newFavorites);

        // Save to localStorage
        localStorage.setItem('favoriteDevices', JSON.stringify(Array.from(newFavorites)));
    };

    // Bulk operations
    const handleBulkAction = async (action: string) => {
        if (selectedDevices.size === 0) return;

        try {
            const deviceIds = Array.from(selectedDevices);
            const successCount = await invoke<number>('bulk_device_action', { action, deviceIds });
            console.log(`✅ Bulk ${action}: ${successCount}/${deviceIds.length} successful`);

            setSelectedDevices(new Set());
            setShowBulkActions(false);
            await loadDevices();

            addToast('success', 'Bulk Operation Complete', `${action} completed on ${successCount} devices`);
        } catch (error) {
            console.error(`❌ Bulk ${action} failed:`, error);
            addToast('error', 'Bulk Operation Failed', `Failed to ${action} devices: ${error}`);
        }
    };

    // Confirmation dialog helpers
    const showConfirmDialog = (
        title: string,
        message: string,
        onConfirm: () => void,
        confirmText: string = 'Confirm',
        variant: 'danger' | 'warning' | 'primary' = 'primary'
    ) => {
        setConfirmDialog({
            isOpen: true,
            title,
            message,
            confirmText,
            confirmVariant: variant,
            onConfirm: () => {
                onConfirm();
                closeConfirmDialog();
            },
            onCancel: closeConfirmDialog
        });
    };

    const closeConfirmDialog = () => {
        setConfirmDialog(prev => ({ ...prev, isOpen: false }));
    };

    // Device selection
    const toggleDeviceSelection = (deviceId: string) => {
        const newSelected = new Set(selectedDevices);
        if (newSelected.has(deviceId)) {
            newSelected.delete(deviceId);
        } else {
            newSelected.add(deviceId);
        }
        setSelectedDevices(newSelected);
        setShowBulkActions(newSelected.size > 0);
    };

    const selectAllDevices = () => {
        const filteredDeviceIds = getFilteredDevices().map(d => d.id);
        setSelectedDevices(new Set(filteredDeviceIds));
        setShowBulkActions(filteredDeviceIds.length > 0);
    };

    const clearSelection = () => {
        setSelectedDevices(new Set());
        setShowBulkActions(false);
    };

    // Enhanced device filtering and sorting with debounced search
    const getFilteredDevices = () => {
        let filtered = devices;

        // Apply search filter with debounced term
        if (debouncedSearchTerm) {
            filtered = filtered.filter(device =>
                device.name.toLowerCase().includes(debouncedSearchTerm.toLowerCase()) ||
                (device.display_name && device.display_name.toLowerCase().includes(debouncedSearchTerm.toLowerCase())) ||
                device.address.includes(debouncedSearchTerm) ||
                (device.platform && device.platform.toLowerCase().includes(debouncedSearchTerm.toLowerCase()))
            );
        }

        // Apply type filter
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

        // Apply sorting
        filtered.sort((a, b) => {
            // Favorites first
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
                    return trustOrder[b.trust_level] - trustOrder[a.trust_level];
                default:
                    return 0;
            }
        });

        return filtered;
    };

    // Device icon helper
    const getDeviceIcon = (deviceType: string, platform: string | undefined, isConnected: boolean) => {
        const iconClass = `w-6 h-6 ${isConnected ? 'text-green-500' : 'text-gray-400'}`;

        switch (deviceType.toLowerCase()) {
            case 'phone':
            case 'mobile':
                return <Smartphone className={iconClass} />;
            case 'tablet':
                return <Tablet className={iconClass} />;
            case 'laptop':
                return <Laptop className={iconClass} />;
            case 'server':
                return <Server className={iconClass} />;
            case 'desktop':
            default:
                return <Monitor className={iconClass} />;
        }
    };

    // Trust level indicator
    const getTrustLevelIndicator = (trustLevel: TrustLevel) => {
        switch (trustLevel) {
            case 'Verified':
                return <ShieldCheck className="w-4 h-4 text-green-500" />;
            case 'Trusted':
                return <Shield className="w-4 h-4 text-blue-500" />;
            case 'Blocked':
                return <ShieldX className="w-4 h-4 text-red-500" />;
            default:
                return <AlertTriangle className="w-4 h-4 text-yellow-500" />;
        }
    };

    // Time formatting helper
    const getTimeAgo = (timestamp: number) => {
        const now = Math.floor(Date.now() / 1000);
        const diff = now - timestamp;

        if (diff < 60) return 'Just now';
        if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
        if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
        return `${Math.floor(diff / 86400)}d ago`;
    };

    // Enhanced Device Card Component with animations
    const DeviceCard = ({ device }: { device: DeviceInfo }) => {
        const isSelected = selectedDevices.has(device.id);
        const isFavorite = favoriteDevices.has(device.id);
        const stats = deviceStats.get(device.id);

        return (
            <motion.div
                layout
                whileHover={{ y: -2, boxShadow: "0 10px 25px -5px rgba(0, 0, 0, 0.1)" }}
                whileTap={{ scale: 0.98 }}
                className={`bg-white/10 backdrop-blur-sm rounded-lg p-4 border transition-all duration-200 hover:bg-white/15 relative group ${device.is_connected
                    ? 'border-green-500/50 shadow-lg shadow-green-500/10'
                    : device.is_paired
                        ? 'border-blue-500/50'
                        : device.is_blocked
                            ? 'border-red-500/50'
                            : 'border-white/20'
                    } ${isSelected ? 'ring-2 ring-blue-400' : ''} ${isFavorite ? 'ring-1 ring-yellow-400/50' : ''}`}
            >
                {/* Selection checkbox */}
                <div className="absolute top-2 left-2">
                    <motion.button
                        whileHover={{ scale: 1.1 }}
                        whileTap={{ scale: 0.9 }}
                        onClick={() => toggleDeviceSelection(device.id)}
                        className="opacity-0 group-hover:opacity-100 transition-opacity"
                    >
                        {isSelected ? (
                            <CheckSquare className="w-4 h-4 text-blue-400" />
                        ) : (
                            <Square className="w-4 h-4 text-gray-400" />
                        )}
                    </motion.button>
                </div>

                {/* Favorite indicator */}
                {isFavorite && (
                    <motion.div
                        initial={{ scale: 0 }}
                        animate={{ scale: 1 }}
                        className="absolute top-2 left-8"
                    >
                        <Star className="w-4 h-4 text-yellow-400 fill-current" />
                    </motion.div>
                )}

                {/* Device menu */}
                <div className="absolute top-2 right-2">
                    <motion.button
                        whileHover={{ scale: 1.1 }}
                        whileTap={{ scale: 0.9 }}
                        onClick={() => setDeviceMenuOpen(deviceMenuOpen === device.id ? null : device.id)}
                        className="opacity-0 group-hover:opacity-100 transition-opacity p-1 hover:bg-white/10 rounded"
                    >
                        <MoreVertical className="w-4 h-4 text-gray-400" />
                    </motion.button>

                    <AnimatePresence>
                        {deviceMenuOpen === device.id && (
                            <motion.div
                                initial={{ opacity: 0, scale: 0.95, y: -10 }}
                                animate={{ opacity: 1, scale: 1, y: 0 }}
                                exit={{ opacity: 0, scale: 0.95, y: -10 }}
                                className="absolute right-0 top-8 bg-black/90 border border-white/20 rounded-lg py-1 min-w-[150px] z-10"
                            >
                                <button
                                    onClick={() => {
                                        setDeviceDetailsModal({ isOpen: true, device });
                                        setDeviceMenuOpen(null);
                                    }}
                                    className="w-full px-3 py-2 text-left text-sm text-gray-300 hover:bg-white/10 flex items-center space-x-2"
                                >
                                    <Eye className="w-4 h-4" />
                                    <span>View Details</span>
                                </button>

                                {!device.is_paired && !device.is_blocked && (
                                    <button
                                        onClick={() => {
                                            showConfirmDialog(
                                                'Pair Device',
                                                `Pair with ${device.display_name || device.name}?`,
                                                () => handlePairDevice(device.id),
                                                'Pair'
                                            );
                                            setDeviceMenuOpen(null);
                                        }}
                                        className="w-full px-3 py-2 text-left text-sm text-gray-300 hover:bg-white/10 flex items-center space-x-2"
                                    >
                                        <Shield className="w-4 h-4" />
                                        <span>Pair Device</span>
                                    </button>
                                )}

                                {device.is_paired && (
                                    <button
                                        onClick={() => {
                                            showConfirmDialog(
                                                'Unpair Device',
                                                `Unpair ${device.display_name || device.name}? You can always pair again later.`,
                                                () => handleUnpairDevice(device.id),
                                                'Unpair',
                                                'warning'
                                            );
                                            setDeviceMenuOpen(null);
                                        }}
                                        className="w-full px-3 py-2 text-left text-sm text-gray-300 hover:bg-white/10 flex items-center space-x-2"
                                    >
                                        <UserX className="w-4 h-4" />
                                        <span>Unpair</span>
                                    </button>
                                )}

                                {!device.is_blocked ? (
                                    <button
                                        onClick={() => {
                                            showConfirmDialog(
                                                'Block Device',
                                                `Block ${device.display_name || device.name}? This device will be prevented from connecting.`,
                                                () => handleBlockDevice(device.id),
                                                'Block',
                                                'danger'
                                            );
                                            setDeviceMenuOpen(null);
                                        }}
                                        className="w-full px-3 py-2 text-left text-sm text-red-400 hover:bg-red-500/10 flex items-center space-x-2"
                                    >
                                        <Ban className="w-4 h-4" />
                                        <span>Block Device</span>
                                    </button>
                                ) : (
                                    <button
                                        onClick={() => {
                                            handleUnblockDevice(device.id);
                                            setDeviceMenuOpen(null);
                                        }}
                                        className="w-full px-3 py-2 text-left text-sm text-green-400 hover:bg-green-500/10 flex items-center space-x-2"
                                    >
                                        <Check className="w-4 h-4" />
                                        <span>Unblock</span>
                                    </button>
                                )}

                                <button
                                    onClick={() => {
                                        showConfirmDialog(
                                            'Forget Device',
                                            `Permanently forget ${device.display_name || device.name}? All history and settings will be lost.`,
                                            () => handleForgetDevice(device.id),
                                            'Forget',
                                            'danger'
                                        );
                                        setDeviceMenuOpen(null);
                                    }}
                                    className="w-full px-3 py-2 text-left text-sm text-red-400 hover:bg-red-500/10 flex items-center space-x-2"
                                >
                                    <Trash2 className="w-4 h-4" />
                                    <span>Forget Device</span>
                                </button>
                            </motion.div>
                        )}
                    </AnimatePresence>
                </div>

                <div className="flex items-center justify-between mb-3 mt-4">
                    <div className="flex items-center space-x-3">
                        <motion.div
                            whileHover={{ scale: 1.1 }}
                            transition={{ type: "spring", stiffness: 400, damping: 10 }}
                        >
                            {getDeviceIcon(device.device_type, device.platform, device.is_connected)}
                        </motion.div>
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
                                        maxLength={50}
                                    />
                                    <motion.button
                                        whileHover={{ scale: 1.1 }}
                                        whileTap={{ scale: 0.9 }}
                                        onClick={() => handleRenameDevice(device.id)}
                                        className="text-green-400 hover:text-green-300"
                                    >
                                        <Check className="w-4 h-4" />
                                    </motion.button>
                                    <motion.button
                                        whileHover={{ scale: 1.1 }}
                                        whileTap={{ scale: 0.9 }}
                                        onClick={() => {
                                            setEditingDevice(null);
                                            setNewDeviceName('');
                                        }}
                                        className="text-red-400 hover:text-red-300"
                                    >
                                        <X className="w-4 h-4" />
                                    </motion.button>
                                </div>
                            ) : (
                                <div className="flex items-center space-x-2">
                                    <h3 className="text-white font-medium text-sm">
                                        {device.display_name || device.name}
                                    </h3>
                                    {device.display_name && (
                                        <span className="text-xs text-gray-500">({device.name})</span>
                                    )}
                                    <motion.button
                                        whileHover={{ scale: 1.1 }}
                                        whileTap={{ scale: 0.9 }}
                                        onClick={() => {
                                            setEditingDevice(device.id);
                                            setNewDeviceName(device.display_name || device.name);
                                        }}
                                        className="text-gray-400 hover:text-white opacity-0 group-hover:opacity-100 transition-opacity"
                                    >
                                        <Edit2 className="w-3 h-3" />
                                    </motion.button>
                                </div>
                            )}
                            <div className="flex items-center space-x-2 mt-1">
                                {device.platform && (
                                    <span className="text-xs text-gray-400">{device.platform}</span>
                                )}
                                <span className="text-xs text-gray-500">•</span>
                                <span className="text-xs text-gray-400">{device.address}</span>
                                <span className="text-xs text-gray-500">•</span>
                                <span className="text-xs text-gray-400">{getTimeAgo(device.last_seen)}</span>
                            </div>
                        </div>
                    </div>
                    <div className="flex items-center space-x-2">
                        <motion.div
                            whileHover={{ scale: 1.1 }}
                            transition={{ type: "spring", stiffness: 400, damping: 10 }}
                        >
                            {getTrustLevelIndicator(device.trust_level)}
                        </motion.div>
                        {device.is_connected ? (
                            <motion.div
                                initial={{ scale: 0 }}
                                animate={{ scale: 1 }}
                                className="flex items-center space-x-1"
                            >
                                <CheckCircle className="w-4 h-4 text-green-500" />
                                <span className="text-xs text-green-400">Connected</span>
                            </motion.div>
                        ) : device.is_blocked ? (
                            <motion.div
                                initial={{ scale: 0 }}
                                animate={{ scale: 1 }}
                                className="flex items-center space-x-1"
                            >
                                <Ban className="w-4 h-4 text-red-500" />
                                <span className="text-xs text-red-400">Blocked</span>
                            </motion.div>
                        ) : device.is_paired ? (
                            <motion.div
                                initial={{ scale: 0 }}
                                animate={{ scale: 1 }}
                                className="flex items-center space-x-1"
                            >
                                <Clock className="w-4 h-4 text-blue-500" />
                                <span className="text-xs text-blue-400">Paired</span>
                            </motion.div>
                        ) : (
                            <motion.div
                                initial={{ scale: 0 }}
                                animate={{ scale: 1 }}
                                className="flex items-center space-x-1"
                            >
                                <AlertCircle className="w-4 h-4 text-gray-500" />
                                <span className="text-xs text-gray-400">Available</span>
                            </motion.div>
                        )}
                    </div>
                </div>

                <div className="flex items-center justify-between">
                    <div className="flex items-center space-x-3 text-xs text-gray-400">
                        <div className="flex items-center space-x-1">
                            <Globe className="w-3 h-3" />
                            <span>{device.version}</span>
                        </div>
                        {device.connection_count > 0 && (
                            <div className="flex items-center space-x-1">
                                <Wifi className="w-3 h-3" />
                                <span>{device.connection_count} connections</span>
                            </div>
                        )}
                        {device.total_transfers > 0 && (
                            <div className="flex items-center space-x-1">
                                <Star className="w-3 h-3" />
                                <span>{device.total_transfers} transfers</span>
                            </div>
                        )}
                    </div>

                    <div className="flex space-x-2">
                        {!device.is_blocked && !device.is_paired && (
                            <motion.button
                                whileHover={{ scale: 1.05 }}
                                whileTap={{ scale: 0.95 }}
                                onClick={() => handlePairDevice(device.id)}
                                className="text-xs px-3 py-1 bg-blue-500/20 text-blue-300 rounded hover:bg-blue-500/30 transition-colors"
                            >
                                Pair
                            </motion.button>
                        )}
                        {device.is_paired && (
                            <motion.button
                                whileHover={{ scale: 1.05 }}
                                whileTap={{ scale: 0.95 }}
                                onClick={() => showConfirmDialog(
                                    'Unpair Device',
                                    `Unpair ${device.display_name || device.name}?`,
                                    () => handleUnpairDevice(device.id),
                                    'Unpair',
                                    'warning'
                                )}
                                className="text-xs px-3 py-1 bg-yellow-500/20 text-yellow-300 rounded hover:bg-yellow-500/30 transition-colors"
                            >
                                Unpair
                            </motion.button>
                        )}
                    </div>
                </div>

                {/* Device Management Section */}
                <motion.div
                    initial={{ height: 0, opacity: 0 }}
                    animate={{ height: 'auto', opacity: 1 }}
                    transition={{ delay: 0.1 }}
                    className="mt-3 pt-3 border-t border-white/10"
                >
                    <DeviceManagement
                        deviceId={device.id}
                        deviceName={device.display_name || device.name}
                        isFavorite={isFavorite}
                        onToggleFavorite={handleToggleFavorite}
                        onConnectDevice={handleConnectDevice}
                        onDisconnectDevice={handleDisconnectDevice}
                        isConnected={device.is_connected}
                        stats={stats}
                    />
                </motion.div>
            </motion.div>
        );
    };

    // Filter and search controls
    const DeviceControls = () => (
        <motion.div
            initial={{ opacity: 0, y: 20 }}
            animate={{ opacity: 1, y: 0 }}
            className="space-y-3 mb-4"
        >
            {/* Search and filter row */}
            <div className="flex items-center space-x-2">
                <div className="relative flex-1">
                    <Search className="w-4 h-4 absolute left-2 top-1/2 transform -translate-y-1/2 text-gray-400" />
                    <input
                        type="text"
                        placeholder="Search devices..."
                        value={searchTerm}
                        onChange={(e) => setSearchTerm(e.target.value)}
                        className="w-full pl-8 pr-3 py-2 bg-black/20 text-white text-sm rounded border border-white/20 focus:outline-none focus:border-blue-400 transition-colors"
                    />
                </div>

                <div className="relative">
                    <select
                        value={filterType}
                        onChange={(e) => setFilterType(e.target.value as any)}
                        className="bg-black/20 text-white text-sm px-3 py-2 rounded border border-white/20 focus:outline-none focus:border-blue-400 appearance-none pr-8"
                    >
                        <option value="all">All Devices</option>
                        <option value="paired">Paired</option>
                        <option value="connected">Connected</option>
                        <option value="blocked">Blocked</option>
                    </select>
                    <ChevronDown className="w-4 h-4 absolute right-2 top-1/2 transform -translate-y-1/2 text-gray-400 pointer-events-none" />
                </div>

                <div className="relative">
                    <select
                        value={sortBy}
                        onChange={(e) => setSortBy(e.target.value as any)}
                        className="bg-black/20 text-white text-sm px-3 py-2 rounded border border-white/20 focus:outline-none focus:border-blue-400 appearance-none pr-8"
                    >
                        <option value="name">Name</option>
                        <option value="last_seen">Last Seen</option>
                        <option value="trust_level">Trust Level</option>
                    </select>
                    <ChevronDown className="w-4 h-4 absolute right-2 top-1/2 transform -translate-y-1/2 text-gray-400 pointer-events-none" />
                </div>
            </div>

            {/* Bulk actions */}
            <AnimatePresence>
                {showBulkActions && (
                    <motion.div
                        initial={{ opacity: 0, height: 0 }}
                        animate={{ opacity: 1, height: 'auto' }}
                        exit={{ opacity: 0, height: 0 }}
                        className="flex items-center justify-between bg-blue-500/10 border border-blue-500/20 rounded-lg p-3"
                    >
                        <span className="text-sm text-blue-300">
                            {selectedDevices.size} device{selectedDevices.size !== 1 ? 's' : ''} selected
                        </span>
                        <div className="flex space-x-2">
                            <motion.button
                                whileHover={{ scale: 1.05 }}
                                whileTap={{ scale: 0.95 }}
                                onClick={() => handleBulkAction('pair')}
                                className="text-xs px-3 py-1 bg-blue-500/20 text-blue-300 rounded hover:bg-blue-500/30 transition-colors"
                            >
                                Pair All
                            </motion.button>
                            <motion.button
                                whileHover={{ scale: 1.05 }}
                                whileTap={{ scale: 0.95 }}
                                onClick={() => handleBulkAction('unpair')}
                                className="text-xs px-3 py-1 bg-yellow-500/20 text-yellow-300 rounded hover:bg-yellow-500/30 transition-colors"
                            >
                                Unpair All
                            </motion.button>
                            <motion.button
                                whileHover={{ scale: 1.05 }}
                                whileTap={{ scale: 0.95 }}
                                onClick={() => showConfirmDialog(
                                    'Block Devices',
                                    `Block ${selectedDevices.size} selected device${selectedDevices.size !== 1 ? 's' : ''}?`,
                                    () => handleBulkAction('block'),
                                    'Block All',
                                    'danger'
                                )}
                                className="text-xs px-3 py-1 bg-red-500/20 text-red-300 rounded hover:bg-red-500/30 transition-colors"
                            >
                                Block All
                            </motion.button>
                            <motion.button
                                whileHover={{ scale: 1.05 }}
                                whileTap={{ scale: 0.95 }}
                                onClick={clearSelection}
                                className="text-xs px-3 py-1 bg-gray-500/20 text-gray-300 rounded hover:bg-gray-500/30 transition-colors"
                            >
                                Clear
                            </motion.button>
                        </div>
                    </motion.div>
                )}
            </AnimatePresence>
        </motion.div>
    );

    const filteredDevices = getFilteredDevices();

    return (
        <div ref={appRef} className="w-full h-full bg-gradient-to-br from-slate-900/95 to-slate-800/95 backdrop-blur-xl rounded-lg border border-white/10 shadow-2xl relative">
            <LoadingOverlay isVisible={isLoading && devices.length === 0} message="Loading devices..." />

            {/* Close window on click outside */}
            {(deviceMenuOpen || confirmDialog.isOpen || deviceDetailsModal.isOpen) && (
                <div
                    className="fixed inset-0 z-0"
                    onClick={() => {
                        setDeviceMenuOpen(null);
                        if (confirmDialog.isOpen) closeConfirmDialog();
                        if (deviceDetailsModal.isOpen) setDeviceDetailsModal({ isOpen: false, device: null });
                    }}
                />
            )}

            {/* Close button */}
            <div className="absolute top-2 right-2 z-10">
                <motion.button
                    whileHover={{ scale: 1.1 }}
                    whileTap={{ scale: 0.9 }}
                    onClick={() => invoke('hide_window')}
                    className="text-gray-400 hover:text-white p-1 rounded hover:bg-white/10 transition-colors"
                >
                    <X className="w-4 h-4" />
                </motion.button>
            </div>

            {/* Header */}
            <FadeIn>
                <div className="p-4 border-b border-white/10">
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
                            <AnimatePresence>
                                {isLoading && (
                                    <motion.div
                                        initial={{ scale: 0 }}
                                        animate={{ scale: 1 }}
                                        exit={{ scale: 0 }}
                                        className="w-3 h-3 border border-blue-400 border-t-transparent rounded-full animate-spin"
                                    />
                                )}
                            </AnimatePresence>
                        </div>
                        <div className="flex items-center space-x-3">
                            <motion.button
                                whileHover={{ scale: 1.1, rotate: 180 }}
                                whileTap={{ scale: 0.9 }}
                                onClick={handleRefresh}
                                disabled={isRefreshing}
                                className="text-gray-400 hover:text-white p-1 rounded hover:bg-white/10 transition-colors"
                            >
                                <RefreshCw className={`w-4 h-4 ${isRefreshing ? 'animate-spin' : ''}`} />
                            </motion.button>
                            {filteredDevices.length > 1 && (
                                <motion.button
                                    whileHover={{ scale: 1.1 }}
                                    whileTap={{ scale: 0.9 }}
                                    onClick={selectAllDevices}
                                    className="text-gray-400 hover:text-white p-1 rounded hover:bg-white/10 transition-colors"
                                >
                                    <CheckSquare className="w-4 h-4" />
                                </motion.button>
                            )}
                            {settings && (
                                <span className="text-xs text-gray-400">{settings.device_name}</span>
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
            </FadeIn>

            {/* Tab Navigation */}
            <SlideIn direction="up">
                <div className="flex border-b border-white/10">
                    {[
                        { id: 'devices', label: 'Devices', icon: Monitor, count: filteredDevices.length },
                        { id: 'settings', label: 'Settings', icon: Settings },
                        { id: 'info', label: 'Info', icon: Info },
                    ].map(({ id, label, icon: Icon, count }) => (
                        <motion.button
                            key={id}
                            whileHover={{ backgroundColor: 'rgba(255, 255, 255, 0.05)' }}
                            whileTap={{ scale: 0.98 }}
                            onClick={() => setActiveTab(id as typeof activeTab)}
                            className={`flex-1 px-4 py-3 text-sm font-medium transition-colors flex items-center justify-center space-x-2 ${activeTab === id
                                ? 'text-blue-400 border-b-2 border-blue-400 bg-blue-500/10'
                                : 'text-gray-400 hover:text-white'
                                }`}
                        >
                            <Icon className="w-4 h-4" />
                            <span>{label}</span>
                            {count !== undefined && (
                                <motion.span
                                    initial={{ scale: 0 }}
                                    animate={{ scale: 1 }}
                                    className={`px-1.5 py-0.5 text-xs rounded-full ${activeTab === id ? 'bg-blue-500/20 text-blue-300' : 'bg-gray-500/20 text-gray-400'
                                        }`}
                                >
                                    {count}
                                </motion.span>
                            )}
                        </motion.button>
                    ))}
                </div>
            </SlideIn>

            {/* Content */}
            <div className="p-4 max-h-[450px] overflow-y-auto">
                <AnimatePresence mode="wait">
                    {activeTab === 'devices' && (
                        <FadeIn key="devices">
                            <div>
                                <DeviceControls />

                                <div className="space-y-3">
                                    {isLoading && devices.length === 0 ? (
                                        <div className="space-y-3">
                                            {[...Array(3)].map((_, i) => (
                                                <DeviceCardSkeleton key={i} />
                                            ))}
                                        </div>
                                    ) : filteredDevices.length === 0 ? (
                                        <motion.div
                                            initial={{ opacity: 0, scale: 0.8 }}
                                            animate={{ opacity: 1, scale: 1 }}
                                            className="text-center py-8"
                                        >
                                            <WifiOff className="w-12 h-12 text-gray-400 mx-auto mb-3" />
                                            <p className="text-gray-400 text-sm">
                                                {debouncedSearchTerm || filterType !== 'all'
                                                    ? 'No devices match your filters'
                                                    : 'No devices discovered'
                                                }
                                            </p>
                                            <p className="text-gray-500 text-xs mt-1">
                                                {debouncedSearchTerm || filterType !== 'all'
                                                    ? 'Try adjusting your search or filters'
                                                    : 'Make sure other devices are running Fileshare'
                                                }
                                            </p>
                                            <motion.button
                                                whileHover={{ scale: 1.05 }}
                                                whileTap={{ scale: 0.95 }}
                                                onClick={handleRefresh}
                                                className="mt-3 px-3 py-1 text-xs bg-blue-500/20 text-blue-300 rounded hover:bg-blue-500/30 transition-colors"
                                            >
                                                Refresh
                                            </motion.button>
                                        </motion.div>
                                    ) : (
                                        <>
                                            <motion.div
                                                initial={{ opacity: 0 }}
                                                animate={{ opacity: 1 }}
                                                className="flex items-center justify-between mb-3"
                                            >
                                                <span className="text-xs text-gray-400">
                                                    {devices.filter(d => d.is_connected).length} connected, {devices.filter(d => d.is_paired).length} paired, {devices.filter(d => d.is_blocked).length} blocked
                                                </span>
                                                <span className="text-xs text-gray-500">
                                                    Showing {filteredDevices.length} of {devices.length}
                                                </span>
                                            </motion.div>
                                            <StaggeredList>
                                                {filteredDevices.map((device) => (
                                                    <DeviceCard key={device.id} device={device} />
                                                ))}
                                            </StaggeredList>
                                        </>
                                    )}
                                </div>
                            </div>
                        </FadeIn>
                    )}

                    {activeTab === 'settings' && settings && (
                        <>
                            <FadeIn key="settings">
                                <AdvancedSettings
                                    settings={settings}
                                    onSettingsChange={(newSettings) => {
                                        setSettings(newSettings);
                                        loadDevices();
                                    }}
                                />
                            </FadeIn>
                            <motion.button
                                whileHover={{ scale: 1.05 }}
                                whileTap={{ scale: 0.95 }}
                                onClick={async () => {
                                    try {
                                        setIsLoading(true);
                                        const result = await invoke<string>('test_hotkey_system');
                                        console.log('Hotkey Test Results:', result);
                                        // Show results in a modal or console
                                        alert(`Hotkey test completed. Check console for details:\n\n${result.substring(0, 500)}...`);
                                    } catch (error) {
                                        console.error('Hotkey test failed:', error);
                                        alert(`Hotkey test failed: ${error}`);
                                    } finally {
                                        setIsLoading(false);
                                    }
                                }}
                                className="w-full px-4 py-2 bg-purple-500/20 text-purple-300 rounded hover:bg-purple-500/30 transition-colors"
                            >
                                🧪 Test Hotkey System
                            </motion.button>
                        </>
                    )}

                    {activeTab === 'info' && (
                        <FadeIn key="info">
                            <EnhancedInfo />
                        </FadeIn>
                    )}
                </AnimatePresence>
            </div>

            {/* Floating Transfer Progress Button */}
            <motion.div
                className="absolute bottom-16 right-4"
                initial={{ scale: 0 }}
                animate={{ scale: 1 }}
                transition={{ delay: 0.5, type: "spring" }}
            >
                <FloatingButton
                    onClick={() => setShowTransferProgress(true)}
                    icon={<Download className="w-4 h-4" />}
                    label="Transfers"
                    variant="primary"
                />
            </motion.div>

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

            {/* Enhanced Confirmation Dialog */}
            <AnimatePresence>
                {confirmDialog.isOpen && (
                    <motion.div
                        initial={{ opacity: 0 }}
                        animate={{ opacity: 1 }}
                        exit={{ opacity: 0 }}
                        className="fixed inset-0 bg-black/50 flex items-center justify-center z-50"
                    >
                        <motion.div
                            initial={{ opacity: 0, scale: 0.8, y: 20 }}
                            animate={{ opacity: 1, scale: 1, y: 0 }}
                            exit={{ opacity: 0, scale: 0.8, y: 20 }}
                            className="bg-slate-800 rounded-lg border border-white/20 p-6 max-w-md w-full mx-4"
                        >
                            <h3 className="text-white font-semibold mb-2">{confirmDialog.title}</h3>
                            <p className="text-gray-300 text-sm mb-4">{confirmDialog.message}</p>
                            <div className="flex space-x-3">
                                <motion.button
                                    whileHover={{ scale: 1.02 }}
                                    whileTap={{ scale: 0.98 }}
                                    onClick={confirmDialog.onCancel}
                                    className="flex-1 px-4 py-2 bg-gray-600 text-white rounded hover:bg-gray-700 transition-colors"
                                >
                                    Cancel
                                </motion.button>
                                <motion.button
                                    whileHover={{ scale: 1.02 }}
                                    whileTap={{ scale: 0.98 }}
                                    onClick={confirmDialog.onConfirm}
                                    className={`flex-1 px-4 py-2 rounded transition-colors ${confirmDialog.confirmVariant === 'danger'
                                        ? 'bg-red-600 hover:bg-red-700 text-white'
                                        : confirmDialog.confirmVariant === 'warning'
                                            ? 'bg-yellow-600 hover:bg-yellow-700 text-white'
                                            : 'bg-blue-600 hover:bg-blue-700 text-white'
                                        }`}
                                >
                                    {confirmDialog.confirmText}
                                </motion.button>
                            </div>
                        </motion.div>
                    </motion.div>
                )}
            </AnimatePresence>

            {/* Enhanced Device Details Modal */}
            <AnimatePresence>
                {deviceDetailsModal.isOpen && deviceDetailsModal.device && (
                    <motion.div
                        initial={{ opacity: 0 }}
                        animate={{ opacity: 1 }}
                        exit={{ opacity: 0 }}
                        className="fixed inset-0 bg-black/50 flex items-center justify-center z-50"
                    >
                        <motion.div
                            initial={{ opacity: 0, scale: 0.8, y: 20 }}
                            animate={{ opacity: 1, scale: 1, y: 0 }}
                            exit={{ opacity: 0, scale: 0.8, y: 20 }}
                            className="bg-slate-800 rounded-lg border border-white/20 p-6 max-w-md w-full mx-4 max-h-[80vh] overflow-y-auto"
                        >
                            <div className="flex items-center justify-between mb-4">
                                <h3 className="text-white font-semibold">Device Details</h3>
                                <motion.button
                                    whileHover={{ scale: 1.1 }}
                                    whileTap={{ scale: 0.9 }}
                                    onClick={() => setDeviceDetailsModal({ isOpen: false, device: null })}
                                    className="text-gray-400 hover:text-white"
                                >
                                    <X className="w-5 h-5" />
                                </motion.button>
                            </div>

                            <div className="space-y-4">
                                <div>
                                    <label className="text-sm text-gray-400">Name</label>
                                    <p className="text-white">{deviceDetailsModal.device.display_name || deviceDetailsModal.device.name}</p>
                                    {deviceDetailsModal.device.display_name && (
                                        <p className="text-xs text-gray-500">Original: {deviceDetailsModal.device.name}</p>
                                    )}
                                </div>

                                <div>
                                    <label className="text-sm text-gray-400">Device Type</label>
                                    <div className="flex items-center space-x-2">
                                        {getDeviceIcon(deviceDetailsModal.device.device_type, deviceDetailsModal.device.platform, deviceDetailsModal.device.is_connected)}
                                        <span className="text-white capitalize">{deviceDetailsModal.device.device_type}</span>
                                        {deviceDetailsModal.device.platform && (
                                            <span className="text-gray-400">({deviceDetailsModal.device.platform})</span>
                                        )}
                                    </div>
                                </div>

                                <div>
                                    <label className="text-sm text-gray-400">Status</label>
                                    <div className="flex items-center space-x-2">
                                        {getTrustLevelIndicator(deviceDetailsModal.device.trust_level)}
                                        <span className="text-white">{deviceDetailsModal.device.trust_level}</span>
                                    </div>
                                </div>

                                <div>
                                    <label className="text-sm text-gray-400">Address</label>
                                    <p className="text-white font-mono text-sm">{deviceDetailsModal.device.address}</p>
                                </div>

                                <div>
                                    <label className="text-sm text-gray-400">Version</label>
                                    <p className="text-white">{deviceDetailsModal.device.version}</p>
                                </div>

                                <div className="grid grid-cols-2 gap-4">
                                    <div>
                                        <label className="text-sm text-gray-400">First Seen</label>
                                        <p className="text-white text-sm">{new Date(deviceDetailsModal.device.first_seen * 1000).toLocaleDateString()}</p>
                                    </div>
                                    <div>
                                        <label className="text-sm text-gray-400">Last Seen</label>
                                        <p className="text-white text-sm">{getTimeAgo(deviceDetailsModal.device.last_seen)}</p>
                                    </div>
                                </div>

                                <div className="grid grid-cols-2 gap-4">
                                    <div>
                                        <label className="text-sm text-gray-400">Connections</label>
                                        <p className="text-white">{deviceDetailsModal.device.connection_count}</p>
                                    </div>
                                    <div>
                                        <label className="text-sm text-gray-400">Transfers</label>
                                        <p className="text-white">{deviceDetailsModal.device.total_transfers}</p>
                                    </div>
                                </div>

                                {deviceDetailsModal.device.last_transfer_time && (
                                    <div>
                                        <label className="text-sm text-gray-400">Last Transfer</label>
                                        <p className="text-white text-sm">{getTimeAgo(deviceDetailsModal.device.last_transfer_time)}</p>
                                    </div>
                                )}
                            </div>
                        </motion.div>
                    </motion.div>
                )}
            </AnimatePresence>
        </div>
    );
}

export default App;