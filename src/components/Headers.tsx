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
            className="px-6 py-4 flex items-center justify-between border-b transition-all duration-300"
            style={{ borderColor: theme.colors.border }}
        >
            <div className="flex items-center gap-2">
                <Aperture
                    className="w-4 h-4"
                    style={{ color: theme.colors.accent1 }}
                />
                <h1
                    className="text-xl font-medium"
                    style={{
                        color: theme.colors.text,
                        background: theme.gradients.accent,
                        backgroundClip: 'text',
                        WebkitBackgroundClip: 'text',
                        WebkitTextFillColor: 'transparent'
                    }}
                >
                    Yeet
                </h1>
            </div>
            {deviceName && (
                <div className="flex items-center gap-2">
                    <Laptop2
                        className="w-4 h-4"
                        style={{ color: theme.colors.accent2 }}
                    />
                    <span
                        className="text-sm"
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