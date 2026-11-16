import { readFileSync, existsSync, readdirSync, statSync } from "node:fs";
import { dirname, resolve, relative } from "node:path";
import assert from "node:assert/strict";
import { createScanner, LanguageVariant, SyntaxKind } from "typescript/unstable/ast";

// Core entry points own policy and state. Browser/desktop effects belong in composition adapters.
const groups = {
  parser: ["core", "parserCore", "specLoader", "specRepository"].map(
    (name) => `packages/autocomplete-parser/src/${name}.ts`,
  ),
  completion: [
    "generators",
    "model",
    "transitions",
    "effects",
    "generator-plan",
    "pipeline",
    "insertion",
    "keypress",
    "types",
    "fig-state",
  ].map((name) => `packages/autocomplete-app/src/state/${name}.ts`),
  lifecycle: [
    "runtime",
    "engine",
    "parser/coordinator",
    "parser/cacheScopes",
    "history/repository",
    "suggestions/index",
    "suggestions/sorting",
  ].map((name) => `packages/autocomplete-app/src/${name}.ts`),
};
const files = Object.values(groups).flat();
function inspect(path, source) {
  // Use the toolchain's lexer to ignore comments/string contents and identify type-only edges.
  // The self-checks below fail fast if the pinned TypeScript lexer changes its contract.
  const scanner = createScanner(true, LanguageVariant.Standard, source);
  const tokens = [];
  while (scanner.scan() !== SyntaxKind.EndOfFile)
    tokens.push({
      kind: scanner.getToken(),
      text: scanner.getTokenText(),
      value: scanner.getTokenValue(),
    });
  const imports = [],
    errors = [];
  for (let i = 0; i < tokens.length; i++) {
    const token = tokens[i];
    if (
      token.kind === SyntaxKind.Identifier &&
      ["window", "document", "localStorage", "globalThis"].includes(token.text)
    )
      errors.push(`${path}: inject ${token.text} through a port`);
    if (!["import", "export"].includes(token.text)) continue;
    const next = tokens[i + 1]?.text;
    if (next === "type") continue;
    if (next === "(") {
      if (tokens[i - 1]?.text === "typeof") continue;
      errors.push(`${path}: inject dynamic imports through a loader port`);
      continue;
    }
    if (token.text === "export" && !["*", "{"].includes(next)) continue;
    const end = tokens.findIndex((t, index) => index > i && t.text === ";");
    const clause = tokens.slice(i + 1, end < 0 ? tokens.length : end);
    const sourceToken = clause.find((t) => t.kind === SyntaxKind.StringLiteral);
    if (!sourceToken) continue;
    const from = clause.findIndex((t) => t.text === "from");
    const binding = clause
      .slice(0, from)
      .map((t) => t.text)
      .join(" ");
    if (
      binding.startsWith("{") &&
      binding.endsWith("}") &&
      binding
        .slice(1, -1)
        .split(",")
        .filter((s) => s.trim())
        .every((s) => s.trim().startsWith("type "))
    )
      continue;
    const target = sourceToken.value;
    imports.push(target);
    if (
      /^(react(?:\/|$)|react-dom(?:\/|$)|@fig\/api-bindings(?:\/|$))/.test(target) ||
      target === "@fig/autocomplete-parser" ||
      target === "@fig/autocomplete-parser/browser" ||
      /(?:^|\/)fig\//.test(target)
    )
      errors.push(`${path}: core cannot import runtime adapter ${target}`);
  }
  return { errors, imports };
}
// Verify that the rule rejects both direct global access and hidden facade dependencies. Global access is checked by the matching oxlint override.
assert.equal(
  inspect("example.ts", 'import type {Shell} from "@fig/api-bindings";').errors.length,
  0,
);
assert.equal(
  inspect("example.ts", 'import {Shell} from "@fig/api-bindings"; window.x=1;').errors.length,
  2,
);
assert.equal(inspect("example.ts", 'export * from "@fig/autocomplete-parser";').errors.length, 1);
assert.equal(
  inspect("example.ts", 'const adapter = import("@fig/api-bindings");').errors.length,
  1,
);
const packages = new Map(
  readdirSync("packages").flatMap((name) => {
    const dir = resolve("packages", name);
    const path = resolve(dir, "package.json");
    if (!existsSync(path)) return [];
    const json = JSON.parse(readFileSync(path, "utf8"));
    return [[json.name, { dir, exports: json.exports }]];
  }),
);
function resolveImport(file, target) {
  let base;
  if (target.startsWith(".")) base = resolve(dirname(file), target.replace(/\.js$/, ""));
  else if (target.startsWith("@fig/")) {
    const [scope, name, ...rest] = target.split("/");
    const pkg = packages.get(`${scope}/${name}`);
    if (!pkg) throw new Error(`Unknown workspace dependency ${target}`);
    const key = rest.length ? `./${rest.join("/")}` : ".";
    let exported = typeof pkg.exports === "string" ? pkg.exports : pkg.exports?.[key];
    if (!exported && pkg.exports)
      for (const [pattern, value] of Object.entries(pkg.exports)) {
        const [prefix, suffix] = pattern.split("*");
        if (suffix !== undefined && key.startsWith(prefix) && key.endsWith(suffix))
          exported = value.replace(
            "*",
            key.slice(prefix.length, suffix ? -suffix.length : undefined),
          );
      }
    if (typeof exported !== "string") throw new TypeError(`Cannot resolve ${target} from ${file}`);
    base = resolve(pkg.dir, exported.replace(/\.js$/, ""));
  } else return undefined;
  const path = [base, `${base}.js`, `${base}.ts`, `${base}.tsx`, `${base}/index.ts`].find(
    (candidate) => existsSync(candidate) && statSync(candidate).isFile(),
  );
  if (!path) throw new Error(`Cannot resolve ${target} from ${file}`);
  return path;
}
const seen = new Set();
const errors = [];
function walk(file, stack = []) {
  if (stack.includes(file)) {
    errors.push(
      `core import cycle: ${[...stack, file].map((path) => relative(process.cwd(), path)).join(" -> ")}`,
    );
    return;
  }
  if (seen.has(file)) return;
  const result = inspect(relative(process.cwd(), file), readFileSync(file, "utf8"));
  errors.push(...result.errors);
  for (const target of result.imports) {
    // Report forbidden adapters at the edge, without inspecting their implementation.
    if (
      /^@fig\/api-bindings(?:\/|$)/.test(target) ||
      target === "@fig/autocomplete-parser" ||
      target === "@fig/autocomplete-parser/browser"
    )
      continue;
    const path = resolveImport(file, target);
    if (path) walk(path, [...stack, file]);
  }
  seen.add(file);
}
for (const file of files) walk(resolve(file));
if (errors.length) {
  console.error([...new Set(errors)].join("\n"));
  process.exitCode = 1;
} else
  console.log(
    `Architecture boundaries: ${files.length} roots, ${seen.size} transitive workspace modules; no runtime adapter imports or cycles.`,
  );
