import React from 'react';
import { motion } from 'framer-motion';
import { Monitor, Settings, Info, Shield } from 'lucide-react';
import { useTheme } from '../context/ThemeContext';

interface NavigationProps {
    activeTab: 'devices' | 'pairing' | 'settings' | 'info';
    onTabChange: (tab: 'devices' | 'pairing' | 'settings' | 'info') => void;
    deviceCount: number;
    unpairedDeviceCount: number;
}

const Navigation: React.FC<NavigationProps> = ({ activeTab, onTabChange, deviceCount, unpairedDeviceCount }) => {
    const { theme } = useTheme();

    const tabs = [
        { id: 'devices', label: 'Devices', icon: Monitor, count: deviceCount },
        { id: 'pairing', label: 'Pairing', icon: Shield, count: unpairedDeviceCount > 0 ? unpairedDeviceCount : undefined },
        { id: 'settings', label: 'Settings', icon: Settings, count: undefined },
        { id: 'info', label: 'Info', icon: Info, count: undefined },
    ] as const;

    return (
        <div className="w-full px-6 py-3 mt-2">
            <div className="flex items-center gap-3">
                {tabs.map(({ id, label, icon: Icon, count }) => (
                    <motion.button
                        key={id}
                        whileHover={{ scale: 1.02 }}
                        whileTap={{ scale: 0.98 }}
                        onClick={() => onTabChange(id)}
                        className="flex-1 relative px-4 py-2.5 rounded-xl text-sm font-medium transition-all duration-200 flex items-center justify-center gap-2 backdrop-blur-sm shadow-sm"
                        style={{
                            backgroundColor: activeTab === id
                                ? `${theme.colors.accent1}20`
                                : `${theme.colors.backgroundSecondary}CC`,
                            color: activeTab === id ? theme.colors.accent1 : theme.colors.textSecondary,
                            border: activeTab === id
                                ? `2px solid ${theme.colors.accent1}40`
                                : `2px solid ${theme.colors.accent1}50`,
                        }}
                        onMouseEnter={(e) => {
                            if (activeTab !== id) {
                                e.currentTarget.style.backgroundColor = `${theme.colors.backgroundTertiary}`;
                                e.currentTarget.style.color = theme.colors.text;
                                e.currentTarget.style.borderColor = `${theme.colors.border}`;
                            }
                        }}
                        onMouseLeave={(e) => {
                            if (activeTab !== id) {
                                e.currentTarget.style.backgroundColor = `${theme.colors.backgroundSecondary}CC`;
                                e.currentTarget.style.color = theme.colors.textSecondary;
                                e.currentTarget.style.borderColor = `${theme.colors.border}50`;
                            }
                        }}
                    >
                        <Icon className="w-4 h-4" />
                        <span>{label}</span>
                        {count !== undefined && (
                            <span
                                className="text-xs font-medium"
                                style={{
                                    color: activeTab === id ? theme.colors.accent1 : theme.colors.textTertiary
                                }}
                            >
                                ({count})
                            </span>
                        )}
                    </motion.button>
                ))}
            </div>
        </div>
    );
};

export default Navigation;