import react from "@vitejs/plugin-react";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";

const __dirname = dirname(fileURLToPath(import.meta.url));

function readRootVersion(): string {
  try {
    return readFileSync(join(__dirname, "../../VERSION"), "utf-8").trim();
  } catch {
    return "0.0.0-dev";
  }
}

export default defineConfig({
  plugins: [react()],
  base: "./",
  clearScreen: false,
  define: {
    "import.meta.env.VITE_APP_RELEASE": JSON.stringify(
      process.env.VITE_APP_RELEASE ?? readRootVersion(),
    ),
  },
  server: {
    port: 5174,
    strictPort: true,
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    // Heavy editors/charts are split into their own chunks so they are only
    // fetched/parsed when a panel that needs them is opened, and so a change
    // in app code does not invalidate the cached vendor chunks.
    chunkSizeWarningLimit: 1500,
    rollupOptions: {
      output: {
        manualChunks(id) {
          if (!id.includes("node_modules")) return;
          if (id.includes("@xyflow")) return "xyflow";
          if (id.includes("monaco-editor") || id.includes("@monaco-editor"))
            return "monaco";
          if (id.includes("@xterm")) return "xterm";
          if (id.includes("uplot")) return "uplot";
          if (id.includes("@tanstack")) return "tanstack";
          if (
            id.includes("/react-dom/") ||
            id.includes("/react/") ||
            id.includes("/scheduler/") ||
            id.includes("react-resizable-panels")
          )
            return "react-vendor";
          return "vendor";
        },
      },
    },
  },
});
