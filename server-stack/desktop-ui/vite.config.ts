import path from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const frontendRoot = path.resolve(__dirname, "../frontend");

export default defineConfig(({ mode }) => ({
  root: frontendRoot,
  base: "./",
  define: {
    "import.meta.env.VITE_CONTROL_API_BASE": JSON.stringify(
      process.env.VITE_CONTROL_API_BASE ??
        (mode === "production" ? "http://127.0.0.1:8080" : ""),
    ),
    /** Embedded WebView / Tauri: enables first-run Control API URL step on login. */
    "import.meta.env.VITE_DEPLOY_DESKTOP": JSON.stringify("1"),
  },
  server: {
    port: 5175,
    strictPort: true,
    proxy: {
      "/api": {
        target: "http://127.0.0.1:8080",
        changeOrigin: true,
        ws: true,
        configure(proxy) {
          proxy.on("proxyReq", (proxyReq, req) => {
            if (req.url?.includes("/stream")) {
              proxyReq.setHeader("Accept", "text/event-stream");
              proxyReq.setHeader("X-Accel-Buffering", "no");
            }
          });
        },
      },
      "/health": "http://127.0.0.1:8080",
    },
  },
  build: {
    outDir: path.resolve(__dirname, "dist"),
    emptyOutDir: true,
    rollupOptions: {
      input: {
        main: path.resolve(frontendRoot, "index.html"),
        login: path.resolve(frontendRoot, "login.html"),
      },
    },
  },
}));
