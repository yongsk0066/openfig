import {
  readFileSync,
  existsSync,
  readdirSync,
  statSync,
  mkdtempSync,
  writeFileSync,
  rmSync,
} from "node:fs";
import { dirname, resolve, relative } from "node:path";
import assert from "node:assert/strict";
import { tmpdir } from "node:os";
import { builtinModules } from "node:module";
const nodeBuiltins = new Set(builtinModules.map((name) => name.replace(/^node:/, "")));
import { API } from "typescript/unstable/sync";
import { SyntaxKind as K, isTypeNode } from "typescript/unstable/ast";

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
const ambient = new Set([
  "window",
  "document",
  "localStorage",
  "globalThis",
  "global",
  "self",
  "performance",
  "Date",
  "setTimeout",
  "clearTimeout",
  "setInterval",
  "clearInterval",
  "requestAnimationFrame",
  "cancelAnimationFrame",
  "queueMicrotask",
  "setImmediate",
  "clearImmediate",
  "requestIdleCallback",
  "cancelIdleCallback",
  "process",
  "fetch",
  "crypto",
  "eval",
  "Function",
  "require",
]);
function adapter(target) {
  return (
    nodeBuiltins.has(target) ||
    /^(node:|timers(?:\/|$)|react(?:\/|$)|react-dom(?:\/|$)|@fig\/api-bindings(?:\/|$))/.test(
      target,
    ) ||
    [
      "@fig/autocomplete-parser",
      "@fig/autocomplete-parser/browser",
      "@fig/shared/runtime-timing",
    ].includes(target) ||
    /(?:^|\/)fig\//.test(target)
  );
}
function inspect(project, file) {
  const errors = [],
    imports = [];
  const path = relative(process.cwd(), file.fileName);
  const fail = (message) => errors.push(`${path}: ${message}`);
  function dependency(node) {
    if (!node || node.kind !== K.StringLiteral) return;
    const target = node.text;
    imports.push(target);
    if (adapter(target)) fail(`core cannot import runtime adapter ${target}`);
  }
  function visit(node) {
    // Type queries and import types have no runtime effect.
    if (isTypeNode(node) && node.kind !== K.ExpressionWithTypeArguments) return;
    if (node.kind === K.ImportDeclaration) {
      const clause = node.importClause;
      const bindings = clause?.namedBindings;
      const onlyTypes =
        clause?.isTypeOnly ||
        (!clause?.name &&
          bindings?.kind === K.NamedImports &&
          bindings.elements.length > 0 &&
          bindings.elements.every((item) => item.isTypeOnly));
      if (!onlyTypes) dependency(node.moduleSpecifier);
      return;
    }
    if (node.kind === K.ExportDeclaration) {
      const clause = node.exportClause;
      const onlyTypes =
        node.isTypeOnly ||
        (clause?.kind === K.NamedExports &&
          clause.elements.length > 0 &&
          clause.elements.every((item) => item.isTypeOnly));
      if (!onlyTypes) dependency(node.moduleSpecifier);
      return;
    }
    if (node.kind === K.ImportEqualsDeclaration && !node.isTypeOnly) {
      dependency(node.moduleReference.expression);
      return;
    }
    if (node.kind === K.CallExpression && node.expression.kind === K.ImportKeyword)
      fail("inject dynamic imports through a loader port");
    if (node.kind === K.Identifier && (ambient.has(node.text) || node.text === "Math")) {
      const parent = node.parent;
      // Property names are not variable references. Shorthand values ARE references.
      const propertyName =
        (parent.kind === K.PropertyAccessExpression && parent.name === node) ||
        ([
          K.PropertyAssignment,
          K.MethodDeclaration,
          K.PropertyDeclaration,
          K.PropertySignature,
        ].includes(parent.kind) &&
          parent.name === node);
      if (!propertyName) {
        const symbol =
          parent.kind === K.ShorthandPropertyAssignment
            ? project.checker.getShorthandAssignmentValueSymbol(parent)
            : project.checker.getSymbolAtLocation(node);
        const declarations = symbol?.declarations ?? [];
        const global =
          !declarations.length || declarations.every((d) => /(?:^|\/)node_modules\//i.test(d.path));
        if (global) {
          if (node.text !== "Math") fail(`inject ${node.text} through a port`);
          else {
            const property =
              parent.kind === K.PropertyAccessExpression && parent.expression === node
                ? parent.name.text
                : parent.kind === K.ElementAccessExpression &&
                    parent.expression === node &&
                    parent.argumentExpression.kind === K.StringLiteral
                  ? parent.argumentExpression.text
                  : undefined;
            if (!property || property === "random")
              fail("inject entropy through a port (no ambient Math alias or dynamic access)");
          }
        }
      }
    }
    node.forEachChild(visit);
  }
  visit(file);
  return { errors, imports };
}
// Real bound programs verify local shadowing, shorthand/aliases, type-only edges and effects.
const cases = [
  [false, 'import type {Shell} from "@fig/api-bindings";'],
  [false, 'import {type Shell} from "@fig/api-bindings";'],
  [false, 'type Time = Date; type API = typeof import("@fig/api-bindings");'],
  [false, 'const text = "performance.now()"; // setTimeout(fn, 1)'],
  [
    false,
    "function local(performance: {now(): number}, setTimeout: () => void) { performance.now(); setTimeout(); }",
  ],
  [false, "const x = {performance: 1, Date: 2}; Math.max(1, 2);"],
  [true, 'import {Shell} from "@fig/api-bindings";'],
  [true, 'export * from "@fig/autocomplete-parser";'],
  [true, 'const adapter = import("@fig/api-bindings");'],
  ...[
    "performance.now()",
    "const clock = performance",
    "const x = {performance}",
    "setTimeout(fn, 1)",
    "Math[key]()",
    "const p = process",
    "requestIdleCallback(fn)",
    "Date.now()",
    "new Date()",
    "class X extends Date {}",
    "queueMicrotask(fn)",
    "Math.random()",
    'Math["random"]()',
    "const {random} = Math",
    'import {setTimeout as sleep} from "node:timers/promises";',
    'import * as clock from "perf_hooks";',
    'require("node:timers")',
    'import {runtimeTiming} from "@fig/shared/runtime-timing";',
  ].map((code) => [true, code]),
];
const fixtureDir = mkdtempSync(resolve(tmpdir(), "openfig-architecture-"));
const api = new API({ cwd: process.cwd() });
try {
  writeFileSync(
    resolve(fixtureDir, "tsconfig.json"),
    JSON.stringify({ compilerOptions: { target: "esnext", module: "esnext", strict: true } }),
  );
  const fixtures = cases.map(([, code], index) => {
    const path = resolve(fixtureDir, `${index}.ts`);
    writeFileSync(path, `${code}\nexport {};`);
    return path;
  });
  let snapshot = api.updateSnapshot({
    openFiles: [...files.map((file) => resolve(file)), ...fixtures],
  });
  const boundFile = (path) => {
    let project = snapshot.getDefaultProjectForFile(path);
    let file = project?.program.getSourceFile(path);
    if (!file) {
      const previous = snapshot;
      snapshot = api.updateSnapshot({ openFiles: [path] });
      previous.dispose();
      project = snapshot.getDefaultProjectForFile(path);
      file = project?.program.getSourceFile(path);
    }
    if (!file) throw new Error(`Cannot bind architecture source ${path}`);
    return inspect(project, file);
  };
  for (let i = 0; i < cases.length; i++)
    assert.equal(
      boundFile(fixtures[i]).errors.length > 0,
      cases[i][0],
      `Architecture rule: ${cases[i][1]}`,
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
      if (typeof exported !== "string")
        throw new TypeError(`Cannot resolve ${target} from ${file}`);
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
    const result = boundFile(file);
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
      `Architecture boundaries: ${files.length} roots, ${seen.size} transitive workspace modules; AST and binding checks passed for runtime adapter imports, ambient effects and cycles (not a whole-program determinism proof).`,
    );
} finally {
  api.close();
  rmSync(fixtureDir, { recursive: true, force: true });
}
