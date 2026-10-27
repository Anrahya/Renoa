import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { cpSync, rmSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const rcpBrowserEntry = fileURLToPath(
  new URL("../../clients/typescript/src/browser.ts", import.meta.url),
);

const designSource = fileURLToPath(new URL("../../design", import.meta.url));
const designTarget = resolve(fileURLToPath(new URL("./public", import.meta.url)), "design");

function copyDesign() {
  rmSync(designTarget, { recursive: true, force: true });
  cpSync(designSource, designTarget, { recursive: true });
}

function designAssets() {
  return {
    name: "renoa-design-assets",
    configureServer() { copyDesign(); },
    buildStart() { copyDesign(); },
  };
}

export default defineConfig({
  build: {
    outDir: "dist/client",
    rollupOptions: {
      output: {
        // The map dependency is shared by live and preview pages and caches independently.
        manualChunks(id) { if (id.includes("/node_modules/@xyflow/")) return "agent-map"; },
      },
    },
  },
  optimizeDeps: {
    include: ["react", "react-dom/client"],
  },
  resolve: {
    alias: {
      "@renoa/rcp-client/browser": rcpBrowserEntry,
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },
  server: {
    host: "0.0.0.0",
    allowedHosts: ["terminal.local"],
    proxy: {
      "/v1/host": "http://127.0.0.1:7819",
      "/v1/identity": "http://127.0.0.1:7818",
      "/connect": { target: "ws://127.0.0.1:7818", ws: true },
    },
    warmup: {
      clientFiles: ["./src/main.tsx"],
    },
  },
  test: {
    include: ["src/**/*.test.{ts,tsx}"],
  },
  plugins: [react(), tailwindcss(), designAssets()],
});
