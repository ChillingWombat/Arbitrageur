/** @type {import('tailwindcss').Config} */
export default {
  content: [
    "./index.html",
    "./src/**/*.{js,ts,jsx,tsx}",
  ],
  theme: {
    extend: {
      colors: {
        background: "#0b0e14",
        surface: "#111622",
        "surface-raised": "#171f30",
        "surface-border": "#212d45",
        accent: "#00f0ff",
        "accent-dim": "rgba(0, 240, 255, 0.15)",
        profit: "#10b981",
        "profit-dim": "rgba(16, 185, 129, 0.15)",
        loss: "#f43f5e",
        "loss-dim": "rgba(244, 63, 94, 0.15)",
        warn: "#f59e0b",
        "warn-dim": "rgba(245, 158, 11, 0.15)",
        muted: "#94a3b8"
      },
      fontFamily: {
        mono: ["JetBrains Mono", "Fira Code", "ui-monospace", "monospace"],
        sans: ["Inter", "system-ui", "-apple-system", "sans-serif"]
      }
    },
  },
  plugins: [],
}
