import { defineConfig } from "vite";

export default defineConfig({
  base: "./",
  server: {
    port: 5174,
    proxy: {
      "/api": "http://127.0.0.1:9090",
    },
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
  },
});
