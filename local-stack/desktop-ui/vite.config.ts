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
  },
});
