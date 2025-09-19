export const themes = {
  dark: {
    name: 'dark',
    colors: {
      // Base colors
      background: '#0C0C0C',
      backgroundSecondary: '#1A1A1A',
      backgroundTertiary: '#242424',

      // Text colors
      text: '#FFFFFF',
      textSecondary: '#B3B3B3',
      textTertiary: '#808080',

      // Accent colors
      accent1: '#00FF85', // Neon green
      accent2: '#1E90FF', // Electric blue
      hover: '#FF0099', // Vivid pink

      // UI colors
      border: 'rgba(255, 255, 255, 0.1)',
      borderHover: 'rgba(255, 255, 255, 0.2)',

      // Status colors
      success: '#00FF85',
      warning: '#FFD700',
      error: '#FF4444',
      info: '#1E90FF',

      // Shadows
      shadow: 'rgba(0, 0, 0, 0.5)',
      shadowStrong: 'rgba(0, 0, 0, 0.8)',
    },
    gradients: {
      primary: 'linear-gradient(135deg, #0D0D0D 0%, #1A1A1A 100%)',
      accent: 'linear-gradient(135deg, #00FF85 0%, #1E90FF 100%)',
      hover: 'linear-gradient(135deg, #FF0099 0%, #1E90FF 100%)',
    },
  },
  light: {
    name: 'light',
    colors: {
      // Base colors
      background: '#FFFFFF',
      backgroundSecondary: '#F5F7FA',
      backgroundTertiary: '#E8ECF0',

      // Text colors
      text: '#0D0D0D',
      textSecondary: '#4A4A4A',
      textTertiary: '#808080',

      // Accent colors
      accent1: '#00CC6A', // Softened neon green
      accent2: '#0077FF', // Darker electric blue
      hover: '#FF007A', // Vivid pink for light theme

      // UI colors
      border: 'rgba(13, 13, 13, 0.1)',
      borderHover: 'rgba(13, 13, 13, 0.2)',

      // Status colors
      success: '#00CC6A',
      warning: '#FFA500',
      error: '#FF4444',
      info: '#0077FF',

      // Shadows
      shadow: 'rgba(0, 0, 0, 0.1)',
      shadowStrong: 'rgba(0, 0, 0, 0.2)',
    },
    gradients: {
      primary: 'linear-gradient(135deg, #FFFFFF 0%, #F5F7FA 100%)',
      accent: 'linear-gradient(135deg, #00CC6A 0%, #0077FF 100%)',
      hover: 'linear-gradient(135deg, #FF007A 0%, #0077FF 100%)',
    },
  },
};

export type Theme = typeof themes.dark;
export type ThemeName = keyof typeof themes;