/** @type {import('tailwindcss').Config} */
export default {
  content: [
    "./index.html",
    "./src/**/*.{js,ts,jsx,tsx}",
  ],
  theme: {
    extend: {
      colors: {
        background: "#f8fafc",
        surface: "#ffffff",
        "surface-raised": "#f1f5f9",
        "surface-border": "#e2e8f0",
        border: "#e2e8f0",
        primary: "#2563eb",
        "primary-hover": "#1d4ed8",
        "primary-dim": "#eff6ff",
        accent: "#2563eb",
        "accent-hover": "#1d4ed8",
        "accent-dim": "#eff6ff",
        "accent-line": "#bfdbfe",
        profit: "#059669",
        "profit-dim": "rgba(5, 150, 105, 0.10)",
        "profit-border": "#a7f3d0",
        loss: "#dc2626",
        "loss-dim": "rgba(220, 38, 38, 0.10)",
        "loss-border": "#fecaca",
        warn: "#d97706",
        "warn-dim": "rgba(217, 119, 6, 0.10)",
        "warn-border": "#fde68a",
        muted: "#64748b"
      },
      boxShadow: {
        "2xs": "0 1px 2px 0 rgba(0, 0, 0, 0.03)",
        xs: "0 1px 2px 0 rgba(0, 0, 0, 0.05)"
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
