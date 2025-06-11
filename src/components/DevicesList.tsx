import React from 'react';
import { motion } from 'framer-motion';
import { WifiOff } from 'lucide-react';
import { StaggeredList } from './AnimatedComponents';
import DeviceCard from './DeviceCard';
import DeviceControls from './DeviceControls';
import { DeviceCardSkeleton } from './LoadingStates';

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
    filterType: string;
    selectedDevices: Set<string>;
    showBulkActions: boolean;
    favoriteDevices: Set<string>;
    onSearchChange: (term: string) => void;
    onFilterChange: (filter: string) => void;
    onSortChange: (sort: string) => void;
    onDeviceSelect: (deviceId: string) => void;
    onSelectAll: () => void;
    onClearSelection: () => void;
    onBulkAction: (action: string) => void;
    onRefresh: () => void;
    deviceActions: {
        onPair: (deviceId: string) => void;
        onUnpair: (deviceId: string) => void;
        onBlock: (deviceId: string) => void;
        onUnblock: (deviceId: string) => void;
        onForget: (deviceId: string) => void;
        onRename: (deviceId: string, newName: string) => void;
        onToggleFavorite: (deviceId: string) => void;
        onConnect: (deviceId: string) => void;
        onDisconnect: (deviceId: string) => void;
    };
}

const DevicesList: React.FC<DevicesListProps> = ({
    devices,
    filteredDevices,
    isLoading,
    searchTerm,
    filterType,
    selectedDevices,
    showBulkActions,
    favoriteDevices,
    onSearchChange,
    onFilterChange,
    onSortChange,
    onDeviceSelect,
    onSelectAll,
    onClearSelection,
    onBulkAction,
    onRefresh,
    deviceActions
}) => {
    return (
        <div>
            <DeviceControls
                searchTerm={searchTerm}
                filterType={filterType}
                selectedDevices={selectedDevices}
                showBulkActions={showBulkActions}
                onSearchChange={onSearchChange}
                onFilterChange={onFilterChange}
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
                        className="text-center py-8"
                    >
                        <WifiOff className="w-12 h-12 text-gray-400 mx-auto mb-3" />
                        <p className="text-gray-400 text-sm">
                            {searchTerm || filterType !== 'all'
                                ? 'No devices match your filters'
                                : 'No devices discovered'
                            }
                        </p>
                        <p className="text-gray-500 text-xs mt-1">
                            {searchTerm || filterType !== 'all'
                                ? 'Try adjusting your search or filters'
                                : 'Make sure other devices are running Fileshare'
                            }
                        </p>
                        <motion.button
                            whileHover={{ scale: 1.05 }}
                            whileTap={{ scale: 0.95 }}
                            onClick={onRefresh}
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
                                <DeviceCard
                                    key={device.id}
                                    device={device}
                                    isSelected={selectedDevices.has(device.id)}
                                    isFavorite={favoriteDevices.has(device.id)}
                                    onSelect={() => onDeviceSelect(device.id)}
                                    onPair={() => deviceActions.onPair(device.id)}
                                    onUnpair={() => deviceActions.onUnpair(device.id)}
                                    onBlock={() => deviceActions.onBlock(device.id)}
                                    onUnblock={() => deviceActions.onUnblock(device.id)}
                                    onForget={() => deviceActions.onForget(device.id)}
                                    onRename={(newName) => deviceActions.onRename(device.id, newName)}
                                    onToggleFavorite={() => deviceActions.onToggleFavorite(device.id)}
                                    onConnect={() => deviceActions.onConnect(device.id)}
                                    onDisconnect={() => deviceActions.onDisconnect(device.id)}
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