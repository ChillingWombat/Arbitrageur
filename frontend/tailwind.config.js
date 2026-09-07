/** @type {import('tailwindcss').Config} */
export default {
  content: [
    "./index.html",
    "./src/**/*.{js,ts,jsx,tsx}",
  ],
  theme: {
    extend: {
      colors: {
        background: "#080B14",
        "bg-page": "#080B14",
        "bg-page-2": "#0C1019",
        surface: "rgba(255, 255, 255, 0.04)",
        "surface-raised": "rgba(255, 255, 255, 0.06)",
        "surface-active": "rgba(255, 255, 255, 0.08)",
        "surface-border": "rgba(255, 255, 255, 0.08)",
        "surface-border-strong": "rgba(255, 255, 255, 0.14)",
        border: "rgba(255, 255, 255, 0.08)",
        primary: "#22D3EE",
        "primary-hover": "#06B6D4",
        "primary-dim": "rgba(34, 211, 238, 0.12)",
        accent: "#818CF8",
        "accent-hover": "#6366F1",
        "accent-dim": "rgba(129, 140, 248, 0.14)",
        "accent-cyan": "#22D3EE",
        "accent-indigo": "#818CF8",
        "accent-magenta": "#E879F9",
        profit: "#34D399",
        "profit-dim": "rgba(52, 211, 153, 0.14)",
        "profit-border": "rgba(52, 211, 153, 0.28)",
        loss: "#FB7185",
        "loss-dim": "rgba(251, 113, 133, 0.14)",
        "loss-border": "rgba(251, 113, 133, 0.28)",
        warn: "#FBBF24",
        "warn-dim": "rgba(251, 191, 36, 0.14)",
        "warn-border": "rgba(251, 191, 36, 0.28)",
        info: "#818CF8",
        "info-dim": "rgba(129, 140, 248, 0.14)",
        muted: "rgba(255, 255, 255, 0.40)",
      },
      boxShadow: {
        "2xs": "0 1px 2px 0 rgba(0, 0, 0, 0.40)",
        xs: "0 2px 4px 0 rgba(0, 0, 0, 0.50)",
        "glass-inset": "inset 0 1px 0 rgba(255, 255, 255, 0.08)",
        "glow-accent": "0 0 25px rgba(129, 140, 248, 0.35)",
        "glow-cyan": "0 0 20px rgba(34, 211, 238, 0.35)",
        "glow-profit": "0 0 20px rgba(52, 211, 153, 0.30)",
        "glow-loss": "0 0 20px rgba(251, 113, 133, 0.30)",
        "card-holographic": "0 8px 40px rgba(129, 140, 248, 0.35), inset 0 0 0 1px rgba(255, 255, 255, 0.15)",
      },
      backdropBlur: {
        xs: "2px"
      },
      fontFamily: {
        mono: ["JetBrains Mono", "ui-monospace", "SFMono-Regular", "Menlo", "Monaco", "Consolas", "monospace"],
        sans: ["Inter", "-apple-system", "BlinkMacSystemFont", "Segoe UI", "Roboto", "sans-serif"]
      }
    },
  },
  plugins: [],
}
