import { fileURLToPath, URL } from "node:url";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [react()],
  // Tauri prints its own output next to Vite's.
  clearScreen: false,
  server: {
    host: "127.0.0.1",
    port: 5173,
    strictPort: true,
  },
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    target: "es2022",
  },
  test: {
    environment: "jsdom",
    setupFiles: "./src/test/setup.ts",
    // The e2e specs (`*.spec.ts`) need the app; the e2e helpers' unit
    // tests (`*.test.ts`) do not.
    include: ["src/**/*.test.{ts,tsx}", "tests/e2e/**/*.test.ts"],
    // Half the cores: with one jsdom worker per core, environment setup
    // (about half the run) starved the running tests and role queries ran
    // past their 1s/5s budgets now and then (M28c). The run takes about as
    // long; the tests see steady timing.
    maxWorkers: "50%",
    environmentOptions: {
      jsdom: {
        url: "http://127.0.0.1/",
      },
    },
  },
});
