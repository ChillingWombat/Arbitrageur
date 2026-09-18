/** @type {import('tailwindcss').Config} */
export default {
  content: [
    "./index.html",
    "./src/**/*.{js,ts,jsx,tsx}",
  ],
  theme: {
    extend: {
      colors: {
        background: "#f4f5f8",
        surface: "#ffffff",
        "surface-raised": "#f8f9fb",
        "surface-border": "#e5e8ed",
        border: "#e5e8ed",
        primary: "#1677ff",
        "primary-hover": "#0958d9",
        "primary-dim": "#edf4ff",
        accent: "#1677ff",
        "accent-hover": "#0958d9",
        "accent-dim": "#edf4ff",
        "accent-line": "#bae0ff",
        profit: "#00a86b",
        "profit-dim": "rgba(0, 168, 107, 0.08)",
        "profit-border": "rgba(0, 168, 107, 0.25)",
        loss: "#ef4444",
        "loss-dim": "rgba(239, 68, 68, 0.08)",
        "loss-border": "rgba(239, 68, 68, 0.25)",
        warn: "#f59e0b",
        "warn-dim": "rgba(245, 158, 11, 0.08)",
        "warn-border": "rgba(245, 158, 11, 0.25)",
        muted: "#6c7b8a",
      },
      boxShadow: {
        "2xs": "0 1px 2px 0 rgba(0, 0, 0, 0.03)",
        xs: "0 1px 2px 0 rgba(0, 0, 0, 0.05)",
        subtle: "0 1px 3px rgba(0, 0, 0, 0.04), 0 1px 2px rgba(0, 0, 0, 0.02)",
        card: "0 1px 3px 0 rgba(0, 0, 0, 0.04), 0 1px 2px -1px rgba(0, 0, 0, 0.02)",
        elevated: "0 4px 12px -2px rgba(0, 0, 0, 0.08), 0 2px 6px -1px rgba(0, 0, 0, 0.04)",
        popover: "0 10px 25px -5px rgba(0, 0, 0, 0.1), 0 8px 10px -6px rgba(0, 0, 0, 0.06)",
      },
      backdropBlur: {
        xs: "2px"
      },
      fontFamily: {
        // All type uses Inter Variable — mono resolves to Inter so numeric
        // contexts stay in-family with tabular-nums for alignment.
        mono: [
          '"Inter Variable"',
          '"Inter"',
          "-apple-system",
          "BlinkMacSystemFont",
          "Segoe UI",
          "sans-serif"
        ],
        sans: [
          '"Inter Variable"',
          '"Inter"',
          "-apple-system",
          "BlinkMacSystemFont",
          "Segoe UI",
          "sans-serif"
        ]
      }
    },
  },
  plugins: [],
}
