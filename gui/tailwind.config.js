/** @type {import('tailwindcss').Config} */
export default {
  content: [
    "./index.html",
    "./src/**/*.{js,ts,jsx,tsx}",
  ],
  theme: {
    extend: {
      colors: {
        primary: '#007AFF',
        success: '#34C759',
        warning: '#FF9500',
        error: '#FF3B30',
        sidebar: '#2c2c2e',
      },
      spacing: {
        '18': '4.5rem',
      },
    },
  },
  plugins: [],
};
