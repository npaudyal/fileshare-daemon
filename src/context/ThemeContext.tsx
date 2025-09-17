import React, { createContext, useState, useContext, useEffect, ReactNode } from 'react';
import { themes, Theme, ThemeName } from '../config/theme';

interface ThemeContextType {
  theme: Theme;
  themeName: ThemeName;
  toggleTheme: () => void;
  setTheme: (name: ThemeName) => void;
}

const ThemeContext = createContext<ThemeContextType | undefined>(undefined);

export const ThemeProvider: React.FC<{ children: ReactNode }> = ({ children }) => {
  const [themeName, setThemeName] = useState<ThemeName>(() => {
    // Load theme from localStorage or default to dark
    const saved = localStorage.getItem('theme');
    return (saved as ThemeName) || 'dark';
  });

  const theme = themes[themeName];

  useEffect(() => {
    // Save theme preference to localStorage
    localStorage.setItem('theme', themeName);

    // Apply theme CSS variables to root
    const root = document.documentElement;
    Object.entries(theme.colors).forEach(([key, value]) => {
      root.style.setProperty(`--color-${key}`, value);
    });
    Object.entries(theme.gradients).forEach(([key, value]) => {
      root.style.setProperty(`--gradient-${key}`, value);
    });
    root.setAttribute('data-theme', themeName);
  }, [theme, themeName]);

  const toggleTheme = () => {
    setThemeName(current => current === 'dark' ? 'light' : 'dark');
  };

  const setTheme = (name: ThemeName) => {
    setThemeName(name);
  };

  return (
    <ThemeContext.Provider value={{ theme, themeName, toggleTheme, setTheme }}>
      {children}
    </ThemeContext.Provider>
  );
};

export const useTheme = () => {
  const context = useContext(ThemeContext);
  if (!context) {
    throw new Error('useTheme must be used within a ThemeProvider');
  }
  return context;
};