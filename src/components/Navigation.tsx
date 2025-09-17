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
        <div className="w-full px-4 py-2">
            <div className="flex items-center gap-2 p-1.5 bg-gray-900/70 backdrop-blur-sm rounded-full">
                {tabs.map(({ id, label, icon: Icon, count }) => (
                    <motion.button
                        key={id}
                        whileHover={{ scale: 1.02 }}
                        whileTap={{ scale: 0.98 }}
                        onClick={() => onTabChange(id)}
                        className={`
                            flex-1 relative px-4 py-2 rounded-full text-sm font-medium 
                            transition-all duration-200 flex items-center justify-center gap-1.5
                            ${activeTab === id
                                ? 'bg-cyan-500/20 text-cyan-400 border border-cyan-500/30'
                                : 'text-gray-400 hover:text-gray-300 hover:bg-gray-800/60'
                            }
                        `}
                    >
                        <Icon className="w-4 h-4" />
                        <span>{label}</span>
                        {count !== undefined && (
                            <span className={`
                                text-xs font-medium
                                ${activeTab === id ? 'text-cyan-400' : 'text-gray-500'}
                            `}>
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