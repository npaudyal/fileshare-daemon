import React from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import { Search, ChevronDown } from 'lucide-react';

interface DeviceControlsProps {
    searchTerm: string;
    selectedDevices: Set<string>;
    showBulkActions: boolean;
    onSearchChange: (term: string) => void;
    onSortChange: (sort: string) => void;
    onSelectAll: () => void;
    onClearSelection: () => void;
    onBulkAction: (action: string) => void;
}

const DeviceControls: React.FC<DeviceControlsProps> = ({
    searchTerm,
    selectedDevices,
    showBulkActions,
    onSearchChange,
    onSortChange,
    onClearSelection,
    onBulkAction
}) => {
    return (
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
                        onChange={(e) => onSearchChange(e.target.value)}
                        className="w-full pl-8 pr-3 py-2 bg-black/20 text-white text-sm rounded border border-white/20 focus:outline-none focus:border-blue-400 transition-colors"
                    />
                </div>

<div className="relative">
                    <select
                        onChange={(e) => onSortChange(e.target.value)}
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
                                onClick={() => onBulkAction('pair')}
                                className="text-xs px-3 py-1 bg-blue-500/20 text-blue-300 rounded hover:bg-blue-500/30 transition-colors"
                            >
                                Pair All
                            </motion.button>
                            <motion.button
                                whileHover={{ scale: 1.05 }}
                                whileTap={{ scale: 0.95 }}
                                onClick={() => onBulkAction('block')}
                                className="text-xs px-3 py-1 bg-red-500/20 text-red-300 rounded hover:bg-red-500/30 transition-colors"
                            >
                                Block All
                            </motion.button>
                            <motion.button
                                whileHover={{ scale: 1.05 }}
                                whileTap={{ scale: 0.95 }}
                                onClick={onClearSelection}
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
};

export default DeviceControls;