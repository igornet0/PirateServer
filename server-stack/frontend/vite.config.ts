import path from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import { viteLoginRoutePlugin } from "./vite-plugin-login-route";

const __dirname = path.dirname(fileURLToPath(import.meta.url));

export default defineConfig({
  plugins: [viteLoginRoutePlugin()],
  server: {
    port: 5173,
    proxy: {
      "/api": {
        // Match control-api default bind (127.0.0.1) from start-local-macos-stack / install.
        // [::1] fails when the API is IPv4-only (common on macOS local stack).
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
    outDir: "dist",
    emptyOutDir: true,
    rollupOptions: {
      input: {
        main: path.resolve(__dirname, "index.html"),
        login: path.resolve(__dirname, "login.html"),
      },
    },
  },
});
