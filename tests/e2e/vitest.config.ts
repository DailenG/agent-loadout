import { defineConfig } from "vitest/config";

// End-to-end smoke tests (SPEC 19 item 9): the debug app through
// tauri-driver, against a throwaway home. `npm run test:e2e`.
export default defineConfig({
  test: {
    include: ["tests/e2e/**/*.spec.ts"],
    environment: "node",
    testTimeout: 120_000,
    hookTimeout: 120_000,
    fileParallelism: false,
  },
});
