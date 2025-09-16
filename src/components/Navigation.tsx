import React from 'react';
import { motion } from 'framer-motion';
import { Monitor, Settings, Info, Shield } from 'lucide-react';

interface NavigationProps {
    activeTab: 'devices' | 'pairing' | 'settings' | 'info';
    onTabChange: (tab: 'devices' | 'pairing' | 'settings' | 'info') => void;
    deviceCount: number;
    unpairedDeviceCount: number;
}

const Navigation: React.FC<NavigationProps> = ({ activeTab, onTabChange, deviceCount, unpairedDeviceCount }) => {
    const tabs = [
        { id: 'devices', label: 'Devices', icon: Monitor, count: deviceCount },
        { id: 'pairing', label: 'Pairing', icon: Shield, count: unpairedDeviceCount > 0 ? unpairedDeviceCount : undefined },
        { id: 'settings', label: 'Settings', icon: Settings, count: undefined },
        { id: 'info', label: 'Info', icon: Info, count: undefined },
    ] as const;

    return (
        <div className="flex border-b border-white/10">
            {tabs.map(({ id, label, icon: Icon, count }) => (
                <motion.button
                    key={id}
                    whileHover={{ backgroundColor: 'rgba(255, 255, 255, 0.05)' }}
                    whileTap={{ scale: 0.98 }}
                    onClick={() => onTabChange(id)}
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
    );
};

export default Navigation;