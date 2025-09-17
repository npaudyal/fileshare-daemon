import React from 'react';
import { Aperture, Laptop2 } from 'lucide-react';

interface HeaderProps {
    deviceName?: string;
}

const Header: React.FC<HeaderProps> = ({ deviceName }) => {
    return (
        <div className="px-6 py-4 flex items-center justify-between">
            <div className="flex items-center gap-2 text-gray-400">

                <Aperture className="w-4 h-4" />

                <h1 className="text-xl font-medium text-white">Yeet</h1>
            </div>
            {deviceName && (
                <div className="flex items-center gap-2 text-gray-400">
                    <Laptop2 className="w-4 h-4" />
                    <span className="text-sm">{deviceName}</span>
                </div>
            )}
        </div>
    );
};

export default Header;