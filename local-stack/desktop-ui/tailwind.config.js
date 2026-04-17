/** @type {import('tailwindcss').Config} */
export default {
  darkMode: "class",
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      fontFamily: {
        sans: ["DM Sans", "system-ui", "Segoe UI", "sans-serif"],
        /** Только логотип / заголовки — как на иконке */
        display: ["Creepster", "cursive"],
      },
      colors: {
        /** Почти чёрный с красным подтоном (как фон иконки) */
        app: "#050204",
        surface: {
          DEFAULT: "#12060a",
          raised: "#1c0a10",
          border: "rgba(220, 38, 38, 0.18)",
        },
        panel: {
          DEFAULT: "#0c0408",
          raised: "#14080e",
        },
        /** Кроваво-красная кромка панелей */
        "border-subtle": "rgba(185, 28, 28, 0.22)",
        deep: "#030102",
        accent: {
          blood: "#b91c1c",
          crimson: "#dc2626",
          flame: "#ea580c",
          from: "#991b1b",
          to: "#ea580c",
        },
      },
      boxShadow: {
        card: "0 4px 20px rgba(0,0,0,0.55), 0 0 0 1px rgba(127, 29, 29, 0.25), inset 0 1px 0 rgba(255,255,255,0.03)",
        glow: "0 0 32px rgba(220, 38, 38, 0.22), 0 0 56px rgba(234, 88, 12, 0.08)",
        flame: "0 0 24px rgba(234, 88, 12, 0.35)",
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
          "50%": { opacity: "0.88" },
        },
      },
      animation: {
        shimmer: "shimmer 1.2s ease-in-out infinite",
        "pulse-soft": "pulseSoft 1.5s ease-in-out infinite",
        flicker: "flicker 4s ease-in-out infinite",
      },
      /** Stacking: modals ≤ modalBlocking; sidebar above overlays so nav stays usable. */
      zIndex: {
        modal: 40,
        modalElevated: 45,
        modalConfirm: 50,
        modalToolchain: 60,
        modalServerSettings: 80,
        modalNested: 90,
        modalNestedHigh: 95,
        modalBlocking: 100,
        /** Main app sidebar — above full-screen modal backdrops (e.g. server project card). */
        sidebar: 110,
      },
    },
  },
  plugins: [],
};
