import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import { fileURLToPath } from "node:url";

const rcpBrowserEntry = fileURLToPath(
  new URL("../../clients/typescript/src/browser.ts", import.meta.url),
);

export default defineConfig({
  build: {
    outDir: "dist/client",
  },
  optimizeDeps: {
    include: ["react", "react-dom/client"],
  },
  resolve: {
    alias: {
      "@renoa/rcp-client/browser": rcpBrowserEntry,
    },
  },
  server: {
    host: "0.0.0.0",
    allowedHosts: ["terminal.local"],
    warmup: {
      clientFiles: ["./src/main.tsx"],
    },
  },
  test: {
    include: ["src/**/*.test.ts"],
  },
  plugins: [react()],
});
