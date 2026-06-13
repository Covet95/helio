/** @type {import('tailwindcss').Config} */
export default {
  darkMode: ["selector", ".dark"],
  content: ["./index.html", "./src/**/*.{js,ts,jsx,tsx}"],
  theme: {
    extend: {
      colors: {
        // surface elevator (dark-first, 4 levels)
        bg: "#0E0E11",
        surface: "#16161A",
        card: "#1C1C22",
        elevated: "#24242C",
        line: "#2A2A33",
        "line-strong": "#3A3A45",

        ink: "#F2F2F5",
        "ink-dim": "#A0A0AC",
        "ink-faint": "#65656F",

        accent: "#3B82F6",
        "accent-soft": "#60A5FA",

        ok: "#22C55E",
        warn: "#F59E0B",
        danger: "#EF4444",

        // per-tool brand accents
        claude: "#D97757",
        codex: "#10B981",
        gemini: "#4F8DF6",
        opencode: "#A78BFA",

        // legacy aliases (keep old class names working)
        primary: "#3B82F6",
        success: "#22C55E",
        warning: "#F59E0B",
        error: "#EF4444",
        sidebar: "#16161A",
      },
      fontFamily: {
        sans: ["Inter", "-apple-system", "BlinkMacSystemFont", "Segoe UI", "sans-serif"],
        mono: ["JetBrains Mono", "ui-monospace", "SFMono-Regular", "SF Mono", "Menlo", "monospace"],
      },
      borderRadius: {
        sm: "0.375rem",
        md: "0.5rem",
        lg: "0.75rem",
        xl: "0.875rem",
        "2xl": "1.125rem",
      },
      boxShadow: {
        soft: "0 1px 2px 0 rgb(0 0 0 / 0.4)",
        card: "0 4px 24px -8px rgb(0 0 0 / 0.5)",
        glow: "0 0 0 1px rgb(59 130 246 / 0.4), 0 8px 32px -8px rgb(59 130 246 / 0.25)",
      },
      keyframes: {
        "fade-up": {
          "0%": { opacity: "0", transform: "translateY(8px)" },
          "100%": { opacity: "1", transform: "translateY(0)" },
        },
        breathe: {
          "0%, 100%": { opacity: "1" },
          "50%": { opacity: "0.35" },
        },
        scan: {
          "0%": { transform: "translateX(-100%)" },
          "100%": { transform: "translateX(100%)" },
        },
      },
      animation: {
        "fade-up": "fade-up 0.4s cubic-bezier(0.16, 1, 0.3, 1) both",
        breathe: "breathe 2.4s ease-in-out infinite",
        scan: "scan 1.5s ease-in-out infinite",
      },
    },
  },
  plugins: [],
};
