import React from 'react';
import { motion } from 'framer-motion';
import { WifiOff } from 'lucide-react';
import { StaggeredList } from './AnimatedComponents';
import DeviceCard from './DeviceCard';
import DeviceControls from './DeviceControls';
import { DeviceCardSkeleton } from './LoadingStates';
import { useTheme } from '../context/ThemeContext';

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

interface DevicesListProps {
    devices: DeviceInfo[];
    filteredDevices: DeviceInfo[];
    isLoading: boolean;
    searchTerm: string;
    selectedDevices: Set<string>;
    showBulkActions: boolean;
    favoriteDevices: Set<string>;
    onSearchChange: (term: string) => void;
    onSortChange: (sort: string) => void;
    onDeviceSelect: (deviceId: string) => void;
    onSelectAll: () => void;
    onClearSelection: () => void;
    onBulkAction: (action: string) => void;
    onRefresh: () => void;
    deviceActions: {
        onPair: (deviceId: string) => void;
        onBlock: (deviceId: string) => void;
        onUnblock: (deviceId: string) => void;
        onForget: (deviceId: string) => void;
        onRename: (deviceId: string, newName: string) => void;
        onToggleFavorite: (deviceId: string) => void;
    };
}

const DevicesList: React.FC<DevicesListProps> = ({
    devices,
    filteredDevices,
    isLoading,
    searchTerm,
    selectedDevices,
    showBulkActions,
    favoriteDevices,
    onSearchChange,
    onSortChange,
    onDeviceSelect,
    onSelectAll,
    onClearSelection,
    onBulkAction,
    onRefresh,
    deviceActions
}) => {
    const { theme } = useTheme();
    return (
        <div>
            <DeviceControls
                searchTerm={searchTerm}
                selectedDevices={selectedDevices}
                showBulkActions={showBulkActions}
                onSearchChange={onSearchChange}
                onSortChange={onSortChange}
                onSelectAll={onSelectAll}
                onClearSelection={onClearSelection}
                onBulkAction={onBulkAction}
            />

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
                        className="text-center py-8 mt-16"
                    >
                        <WifiOff className="w-12 h-12 mx-auto mb-3" style={{ color: theme.colors.textSecondary }} />
                        <p className="text-sm" style={{ color: theme.colors.textSecondary }}>
                            {searchTerm
                                ? 'No devices match your search'
                                : 'No paired devices'
                            }
                        </p>
                        <p className="text-xs mt-1" style={{ color: theme.colors.textTertiary }}>
                            {searchTerm
                                ? 'Try adjusting your search'
                                : 'Go to the Pairing tab to pair devices'
                            }
                        </p>
                        <motion.button
                            whileHover={{ scale: 1.05 }}
                            whileTap={{ scale: 0.95 }}
                            onClick={onRefresh}
                            className="mt-3 px-3 py-1 text-xs rounded transition-all duration-200"
                            style={{
                                backgroundColor: theme.colors.accent2 + '20',
                                color: theme.colors.accent2,
                                border: `1px solid ${theme.colors.accent2}40`
                            }}
                            onMouseEnter={(e) => {
                                e.currentTarget.style.backgroundColor = theme.colors.accent2 + '30';
                                e.currentTarget.style.boxShadow = `0 0 12px ${theme.colors.accent2}40`;
                            }}
                            onMouseLeave={(e) => {
                                e.currentTarget.style.backgroundColor = theme.colors.accent2 + '20';
                                e.currentTarget.style.boxShadow = 'none';
                            }}
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
                            <span className="text-xs" style={{ color: theme.colors.textSecondary }}>
                                {devices.filter(d => d.is_connected).length} connected
                            </span>
                            <span className="text-xs" style={{ color: theme.colors.textTertiary }}>
                                Showing {filteredDevices.length} paired device{filteredDevices.length !== 1 ? 's' : ''}
                            </span>
                        </motion.div>
                        <StaggeredList>
                            {filteredDevices.map((device) => (
                                <DeviceCard
                                    key={device.id}
                                    device={device}
                                    isSelected={selectedDevices.has(device.id)}
                                    isFavorite={favoriteDevices.has(device.id)}
                                    onSelect={() => onDeviceSelect(device.id)}
                                    onPair={() => deviceActions.onPair(device.id)}
                                    onBlock={() => deviceActions.onBlock(device.id)}
                                    onUnblock={() => deviceActions.onUnblock(device.id)}
                                    onForget={() => deviceActions.onForget(device.id)}
                                    onRename={(newName) => deviceActions.onRename(device.id, newName)}
                                    onToggleFavorite={() => deviceActions.onToggleFavorite(device.id)}
                                />
                            ))}
                        </StaggeredList>
                    </>
                )}
            </div>
        </div>
    );
};

export default DevicesList;