import { defineConfig } from "vite";

export default defineConfig({
  server: {
    port: 5173,
    proxy: {
      "/api": "http://[::1]:8080",
      "/health": "http://[::1]:8080",
    },
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
  },
});
