import { defineConfig } from "vitest/config";

/**
 * One place for every test project. Vitest 5 `test.projects` accepts inline
 * definitions, so the five one-line `vitest.config.ts` files that used to sit
 * in the packages are gone and the environments are visible side by side.
 *
 * `tests/fig-api` is deliberately not a project here: its setup shells out to
 * `cargo build -p fig-api-mock` and every IPC message is a `cargo run`, which
 * makes `vitest run` need a Rust toolchain and takes ~31s instead of ~6s. Run
 * it on demand with
 *   pnpm exec vitest run --root tests/fig-api
 * (`just test` builds the workspace with cargo first anyway).
 */
export default defineConfig({
  test: {
    projects: [
      { test: { name: "shared", root: "packages/shared" } },
      { test: { name: "shell-parser", root: "packages/shell-parser" } },
      {
        test: {
          name: "autocomplete-parser",
          root: "packages/autocomplete-parser",
          environment: "jsdom",
        },
      },
      {
        test: {
          name: "autocomplete-app",
          root: "packages/autocomplete-app",
          environment: "jsdom",
        },
      },
    ],
    // Persist Vite transforms between runs; transforming the autocomplete-parser
    // dependency graph was ~34% of a cold run.
    fsModuleCache: true,
    coverage: {
      provider: "v8",
      reporter: ["text", "json", "html", "lcov"],
      include: ["packages/*/src/**", "packages/shell-parser/src/**"],
      exclude: ["**/tests/**", "**/*.test.ts", "packages/fuzzysort/**", "proto/**"],
    },
  },
});
