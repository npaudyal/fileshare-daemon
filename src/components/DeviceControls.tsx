import React from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import { Search, ChevronDown } from 'lucide-react';
import { useTheme } from '../context/ThemeContext';

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
    const { theme } = useTheme();
    return (
        <motion.div
            initial={{ opacity: 0, y: 20 }}
            animate={{ opacity: 1, y: 0 }}
            className="space-y-3 mb-4"
        >
            {/* Search and filter row */}
            <div className="flex items-center space-x-2">
                <div className="relative flex-1">
                    <Search className="w-4 h-4 absolute left-2 top-1/2 transform -translate-y-1/2" style={{ color: theme.colors.textTertiary }} />
                    <input
                        type="text"
                        placeholder="Search devices..."
                        value={searchTerm}
                        onChange={(e) => onSearchChange(e.target.value)}
                        className="w-full pl-8 pr-3 py-2 text-sm rounded border focus:outline-none transition-all duration-200"
                        style={{
                            backgroundColor: theme.colors.backgroundSecondary,
                            color: theme.colors.text,
                            borderColor: theme.colors.border
                        }}
                        onFocus={(e) => {
                            e.currentTarget.style.borderColor = theme.colors.accent2;
                            e.currentTarget.style.boxShadow = `0 0 0 3px ${theme.colors.accent2}20`;
                        }}
                        onBlur={(e) => {
                            e.currentTarget.style.borderColor = theme.colors.border;
                            e.currentTarget.style.boxShadow = 'none';
                        }}
                    />
                </div>

<div className="relative">
                    <select
                        onChange={(e) => onSortChange(e.target.value)}
                        className="text-sm px-3 py-2 rounded border focus:outline-none appearance-none pr-8 transition-all duration-200"
                        style={{
                            backgroundColor: theme.colors.backgroundSecondary,
                            color: theme.colors.text,
                            borderColor: theme.colors.border
                        }}
                        onFocus={(e) => {
                            e.currentTarget.style.borderColor = theme.colors.accent2;
                            e.currentTarget.style.boxShadow = `0 0 0 3px ${theme.colors.accent2}20`;
                        }}
                        onBlur={(e) => {
                            e.currentTarget.style.borderColor = theme.colors.border;
                            e.currentTarget.style.boxShadow = 'none';
                        }}
                    >
                        <option value="name">Name</option>
                        <option value="last_seen">Last Seen</option>
                        <option value="trust_level">Trust Level</option>
                    </select>
                    <ChevronDown className="w-4 h-4 absolute right-2 top-1/2 transform -translate-y-1/2 pointer-events-none" style={{ color: theme.colors.textTertiary }} />
                </div>
            </div>

            {/* Bulk actions */}
            <AnimatePresence>
                {showBulkActions && (
                    <motion.div
                        initial={{ opacity: 0, height: 0 }}
                        animate={{ opacity: 1, height: 'auto' }}
                        exit={{ opacity: 0, height: 0 }}
                        className="flex items-center justify-between rounded-lg p-3"
                        style={{
                            backgroundColor: theme.colors.accent2 + '15',
                            border: `1px solid ${theme.colors.accent2}30`
                        }}
                    >
                        <span className="text-sm" style={{ color: theme.colors.accent2 }}>
                            {selectedDevices.size} device{selectedDevices.size !== 1 ? 's' : ''} selected
                        </span>
                        <div className="flex space-x-2">
                            <motion.button
                                whileHover={{ scale: 1.05 }}
                                whileTap={{ scale: 0.95 }}
                                onClick={() => onBulkAction('pair')}
                                className="text-xs px-3 py-1 rounded transition-all duration-200"
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
                                Pair All
                            </motion.button>
                            <motion.button
                                whileHover={{ scale: 1.05 }}
                                whileTap={{ scale: 0.95 }}
                                onClick={() => onBulkAction('block')}
                                className="text-xs px-3 py-1 rounded transition-all duration-200"
                                style={{
                                    backgroundColor: theme.colors.error + '20',
                                    color: theme.colors.error,
                                    border: `1px solid ${theme.colors.error}40`
                                }}
                                onMouseEnter={(e) => {
                                    e.currentTarget.style.backgroundColor = theme.colors.error + '30';
                                    e.currentTarget.style.boxShadow = `0 0 12px ${theme.colors.error}40`;
                                }}
                                onMouseLeave={(e) => {
                                    e.currentTarget.style.backgroundColor = theme.colors.error + '20';
                                    e.currentTarget.style.boxShadow = 'none';
                                }}
                            >
                                Block All
                            </motion.button>
                            <motion.button
                                whileHover={{ scale: 1.05 }}
                                whileTap={{ scale: 0.95 }}
                                onClick={onClearSelection}
                                className="text-xs px-3 py-1 rounded transition-all duration-200"
                                style={{
                                    backgroundColor: theme.colors.textSecondary + '20',
                                    color: theme.colors.textSecondary,
                                    border: `1px solid ${theme.colors.textSecondary}40`
                                }}
                                onMouseEnter={(e) => {
                                    e.currentTarget.style.backgroundColor = theme.colors.textSecondary + '30';
                                    e.currentTarget.style.boxShadow = `0 0 12px ${theme.colors.textSecondary}40`;
                                }}
                                onMouseLeave={(e) => {
                                    e.currentTarget.style.backgroundColor = theme.colors.textSecondary + '20';
                                    e.currentTarget.style.boxShadow = 'none';
                                }}
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