import React from 'react';
import { Aperture, Laptop2 } from 'lucide-react';
import { useTheme } from '../context/ThemeContext';

interface HeaderProps {
    deviceName?: string;
}

const Header: React.FC<HeaderProps> = ({ deviceName }) => {
    const { theme } = useTheme();


    return (
        <div
            className="px-10 py-5 flex items-center justify-between border-b transition-all duration-300"
            style={{
                backgroundColor: theme.colors.backgroundSecondary,
                borderColor: theme.colors.border
            }}
        >
            <div className="flex items-center">
                <h1
                    className="text-4xl"
                    style={{
                        fontFamily: '"Gasoek One", sans-serif',
                        fontWeight: 400,
                        color: theme.colors.text
                    }}
                >
                    Yeet
                </h1>
            </div>
            {deviceName && (
                <div className="flex items-center gap-2">
                    <Laptop2
                        className="w-5 h-5"
                        style={{ color: theme.colors.textSecondary }}
                    />
                    <span
                        className="text-sm font-medium"
                        style={{ color: theme.colors.textSecondary }}
                    >
                        {deviceName}
                    </span>
                </div>
            )}
        </div>
    );
};

export default Header;