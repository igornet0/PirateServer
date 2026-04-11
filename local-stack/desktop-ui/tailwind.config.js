/** @type {import('tailwindcss').Config} */
export default {
  darkMode: "class",
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      fontFamily: {
        sans: ["DM Sans", "system-ui", "Segoe UI", "sans-serif"],
        display: ["Creepster", "cursive"],
      },
      colors: {
        surface: {
          DEFAULT: "#14080c",
          raised: "#1c0c12",
          border: "rgba(220, 38, 38, 0.12)",
        },
        deep: "#050204",
        accent: {
          from: "#b91c1c",
          to: "#ea580c",
        },
      },
      boxShadow: {
        card: "0 4px 24px rgba(0,0,0,0.5), 0 0 0 1px rgba(220, 38, 38, 0.08)",
        glow: "0 0 48px rgba(185, 28, 28, 0.18), 0 0 80px rgba(234, 88, 12, 0.06)",
      },
      keyframes: {
        shimmer: {
          "100%": { transform: "translateX(100%)" },
        },
        pulseSoft: {
          "0%, 100%": { opacity: "0.5" },
          "50%": { opacity: "0.9" },
        },
        flicker: {
          "0%, 100%": { opacity: "1" },
          "50%": { opacity: "0.92" },
        },
      },
      animation: {
        shimmer: "shimmer 1.2s ease-in-out infinite",
        "pulse-soft": "pulseSoft 1.5s ease-in-out infinite",
        flicker: "flicker 4s ease-in-out infinite",
      },
    },
  },
  plugins: [],
};
