import React from 'react';
import { Moon, Sun } from 'lucide-react';
import { motion } from 'framer-motion';
import { useTheme } from '../context/ThemeContext';

const ThemeToggle: React.FC = () => {
  const { themeName, toggleTheme, theme } = useTheme();

  return (
    <motion.button
      whileHover={{ scale: 1.05 }}
      whileTap={{ scale: 0.95 }}
      onClick={toggleTheme}
      className="relative p-2 rounded-lg transition-all duration-300"
      style={{
        backgroundColor: theme.colors.backgroundSecondary,
        border: `1px solid ${theme.colors.border}`,
      }}
      onMouseEnter={(e) => {
        e.currentTarget.style.borderColor = theme.colors.accent1;
        e.currentTarget.style.boxShadow = `0 0 12px ${theme.colors.accent1}40`;
      }}
      onMouseLeave={(e) => {
        e.currentTarget.style.borderColor = theme.colors.border;
        e.currentTarget.style.boxShadow = 'none';
      }}
    >
      <div className="flex items-center space-x-2">
        <motion.div
          initial={false}
          animate={{ rotate: themeName === 'dark' ? 0 : 180 }}
          transition={{ duration: 0.3 }}
        >
          {themeName === 'dark' ? (
            <Moon className="w-5 h-5" style={{ color: theme.colors.accent1 }} />
          ) : (
            <Sun className="w-5 h-5" style={{ color: theme.colors.accent2 }} />
          )}
        </motion.div>
        <span className="text-sm font-medium" style={{ color: theme.colors.text }}>
          {themeName === 'dark' ? 'Dark' : 'Light'}
        </span>
      </div>
    </motion.button>
  );
};

export default ThemeToggle;