import React, { useState } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import { useTheme } from '../context/ThemeContext';

// Install framer-motion first: npm install framer-motion

interface FadeInProps {
    children: React.ReactNode;
    delay?: number;
    duration?: number;
    className?: string;
}

export const FadeIn: React.FC<FadeInProps> = ({
    children,
    delay = 0,
    duration = 0.3,
    className = ''
}) => (
    <motion.div
        initial={{ opacity: 0, y: 20 }}
        animate={{ opacity: 1, y: 0 }}
        exit={{ opacity: 0, y: -20 }}
        transition={{ duration, delay }}
        className={className}
    >
        {children}
    </motion.div>
);

interface SlideInProps {
    children: React.ReactNode;
    direction?: 'left' | 'right' | 'up' | 'down';
    delay?: number;
    className?: string;
}

export const SlideIn: React.FC<SlideInProps> = ({
    children,
    direction = 'up',
    delay = 0,
    className = ''
}) => {
    const variants = {
        left: { x: -50, y: 0 },
        right: { x: 50, y: 0 },
        up: { x: 0, y: 20 },
        down: { x: 0, y: -20 }
    };

    return (
        <motion.div
            initial={{ opacity: 0, ...variants[direction] }}
            animate={{ opacity: 1, x: 0, y: 0 }}
            exit={{ opacity: 0, ...variants[direction] }}
            transition={{ duration: 0.3, delay }}
            className={className}
        >
            {children}
        </motion.div>
    );
};

interface ScaleInProps {
    children: React.ReactNode;
    delay?: number;
    className?: string;
}

export const ScaleIn: React.FC<ScaleInProps> = ({ children, delay = 0, className = '' }) => (
    <motion.div
        initial={{ opacity: 0, scale: 0.8 }}
        animate={{ opacity: 1, scale: 1 }}
        exit={{ opacity: 0, scale: 0.8 }}
        transition={{ duration: 0.2, delay }}
        className={className}
    >
        {children}
    </motion.div>
);

interface StaggeredListProps {
    children: React.ReactNode[];
    className?: string;
}

export const StaggeredList: React.FC<StaggeredListProps> = ({ children, className = '' }) => (
    <motion.div className={className}>
        <AnimatePresence>
            {children.map((child, index) => (
                <motion.div
                    key={index}
                    initial={{ opacity: 0, x: -20 }}
                    animate={{ opacity: 1, x: 0 }}
                    exit={{ opacity: 0, x: 20 }}
                    transition={{ duration: 0.2, delay: index * 0.05 }}
                >
                    {child}
                </motion.div>
            ))}
        </AnimatePresence>
    </motion.div>
);

// Floating action button with micro-interactions
interface FloatingButtonProps {
    onClick: () => void;
    icon: React.ReactNode;
    label: string;
    variant?: 'primary' | 'secondary' | 'danger';
    disabled?: boolean;
}

export const FloatingButton: React.FC<FloatingButtonProps> = ({
    onClick,
    icon,
    label,
    variant = 'primary',
    disabled = false
}) => {
    const { theme } = useTheme();
    const [isHovered, setIsHovered] = useState(false);

    const getVariantColors = (variant: string) => {
        switch (variant) {
            case 'primary':
                return {
                    bg: theme.colors.accent2,
                    bgHover: theme.colors.accent2 + 'DD',
                    text: theme.colors.text,
                    shadow: theme.colors.accent2 + '40'
                };
            case 'secondary':
                return {
                    bg: theme.colors.textSecondary,
                    bgHover: theme.colors.textSecondary + 'DD',
                    text: theme.colors.text,
                    shadow: theme.colors.textSecondary + '40'
                };
            case 'danger':
                return {
                    bg: theme.colors.error,
                    bgHover: theme.colors.error + 'DD',
                    text: theme.colors.text,
                    shadow: theme.colors.error + '40'
                };
            default:
                return {
                    bg: theme.colors.accent2,
                    bgHover: theme.colors.accent2 + 'DD',
                    text: theme.colors.text,
                    shadow: theme.colors.accent2 + '40'
                };
        }
    };

    const colors = getVariantColors(variant);

    return (
        <motion.button
            onClick={onClick}
            disabled={disabled}
            onHoverStart={() => setIsHovered(true)}
            onHoverEnd={() => setIsHovered(false)}
            className="relative flex items-center space-x-2 px-4 py-2 rounded-full shadow-lg transition-all duration-200 disabled:opacity-50"
            style={{
                backgroundColor: isHovered ? colors.bgHover : colors.bg,
                color: colors.text
            }}
            whileHover={{ scale: 1.05, y: -2 }}
            whileTap={{ scale: 0.95 }}
            animate={{
                boxShadow: isHovered
                    ? `0 20px 25px -5px ${colors.shadow}, 0 10px 10px -5px ${theme.colors.shadow}`
                    : `0 10px 15px -3px ${colors.shadow}, 0 4px 6px -2px ${theme.colors.shadow}`
            }}
        >
            <motion.div
                animate={{ rotate: isHovered ? 360 : 0 }}
                transition={{ duration: 0.3 }}
            >
                {icon}
            </motion.div>
            <span className="text-sm font-medium">{label}</span>
        </motion.button>
    );
};