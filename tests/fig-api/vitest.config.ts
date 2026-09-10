import { defineConfig } from "vitest/config";

/**
 * `tests/fig-api` is deliberately not one of the projects in the root
 * `vitest.config.ts`: its setup shells out to `cargo build -p fig-api-mock` and
 * every IPC message is a `cargo run`, which would make a plain `vitest run`
 * need a Rust toolchain. It keeps its own config so the on-demand invocation
 *   pnpm exec vitest run --root tests/fig-api
 * gets the jsdom environment and the mock-spawning setup file it needs.
 */
export default defineConfig({
  test: {
    clearMocks: true,
    environment: "jsdom",
    setupFiles: ["src/setup.ts"],
  },
});
