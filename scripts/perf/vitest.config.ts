// On-demand GUI measurements for the performance baseline (docs/perf).
// Not part of `npm test`: run with
//   npx vitest run --config scripts/perf/vitest.config.ts
import { fileURLToPath, URL } from "node:url";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("../../src", import.meta.url)),
    },
  },
  test: {
    root: fileURLToPath(new URL("../..", import.meta.url)),
    environment: "jsdom",
    // The DevTools hook must exist before react-dom loads.
    setupFiles: ["./scripts/perf/renderHook.ts", "./src/test/setup.ts"],
    include: ["scripts/perf/**/*.perf.test.tsx"],
    testTimeout: 120_000,
    environmentOptions: {
      jsdom: {
        url: "http://127.0.0.1/",
      },
    },
  },
});
