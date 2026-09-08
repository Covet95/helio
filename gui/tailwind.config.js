/** @type {import('tailwindcss').Config} */
export default {
  darkMode: ["selector", ".dark"],
  content: ["./index.html", "./src/**/*.{js,ts,jsx,tsx}"],
  theme: {
    extend: {
      colors: {
        bg: "#F7F8FA",
        surface: "#F3F4F6",
        card: "#FFFFFF",
        elevated: "#EDEFF2",
        line: "#E1E4E8",
        "line-strong": "#BBC2CA",

        ink: "#20242B",
        "ink-dim": "#505967",
        "ink-faint": "#687280",

        accent: "#C94B0A",
        "accent-soft": "#EA580C",

        ok: "#15803D",
        warn: "#A16207",
        danger: "#DC2626",

        claude: "#C2410C",
        codex: "#047857",
        pi: "#2563EB",
        opencode: "#7C3AED",

        primary: "#20242B",
        success: "#16A34A",
        warning: "#D97706",
        error: "#DC2626",
        sidebar: "#FFFFFF",
      },
      fontFamily: {
        sans: ["-apple-system", "BlinkMacSystemFont", "Segoe UI", "PingFang SC", "Microsoft YaHei", "sans-serif"],
        mono: ["ui-monospace", "SFMono-Regular", "SF Mono", "Menlo", "Consolas", "monospace"],
      },
      borderRadius: {
        sm: "0.375rem",
        md: "0.5rem",
        lg: "0.5rem",
        xl: "0.5rem",
        "2xl": "0.5rem",
      },
      boxShadow: {
        soft: "0 1px 2px 0 rgb(60 40 15 / 0.05)",
        card: "0 1px 2px 0 rgb(60 40 15 / 0.06), 0 14px 28px -16px rgb(60 40 15 / 0.18)",
        glow: "0 0 0 1px rgb(234 88 12 / 0.20), 0 3px 10px -4px rgb(234 88 12 / 0.24)",
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
