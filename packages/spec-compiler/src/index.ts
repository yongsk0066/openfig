import fs from "node:fs/promises";
import { builtinModules } from "node:module";
import path from "node:path";
import { build, type Plugin } from "esbuild";

/**
 * The `compile` half of `@withfig/autocomplete-tools` (2.11.0), which is all the
 * bundle ever used. One esbuild call turns `src/**\/*.ts` into one minified ESM
 * module per spec, plus the four index files the autocomplete app loads.
 *
 * Dropped on the way in: the `commander` CLI wrapper (`node:util`'s `parseArgs`
 * does it), `chalk` (nothing here is coloured), `fast-glob` (Node 26 has
 * `fs.glob`), chokidar/`--watch` (the app is built, not developed, from here),
 * and `invalidateCache()`, which shelled out to a `q`/`cw`/`fig` binary on every
 * compile to flip a developer setting — that call is what forced the Justfile to
 * scrub `~/.local/bin` out of PATH.
 */

const SOURCE_FOLDER_NAME = "src";

/** `import "fs"` from inside a spec resolves to an empty CommonJS module. */
const NODE_BUILTINS = new Set([...builtinModules, ...builtinModules.map((name) => `node:${name}`)]);

/**
 * Specs run in the autocomplete webview, where no Node builtin exists. Upstream
 * pulled in `@esbuild-plugins/node-modules-polyfill` for this; every import that
 * reaches it here is dead code that esbuild then tree-shakes away (verified
 * against the byte-level baseline), so an empty module is enough. CommonJS, not
 * ESM, so that a named import of a stubbed builtin stays a runtime lookup
 * instead of failing the build.
 */
function nodeBuiltinStubs(stubbed: Set<string>): Plugin {
  return {
    name: "node-builtin-stubs",
    setup(pluginBuild) {
      pluginBuild.onResolve({ filter: /.*/ }, (args) => {
        if (!NODE_BUILTINS.has(args.path)) return null;
        stubbed.add(args.path);
        return { path: args.path, namespace: "node-builtin-stub" };
      });
      pluginBuild.onLoad({ filter: /.*/, namespace: "node-builtin-stub" }, () => ({
        contents: "module.exports = {};",
        loader: "js" as const,
      }));
    },
  };
}

/**
 * `index.json` is what the Rust side reads to know which specs exist;
 * `index.js` / `dynamic/index.js` are what the app imports. Byte-for-byte the
 * same shape as the upstream tool wrote them.
 */
async function generateIndex(outdir: string, srcdir: string, files: string[]) {
  const parsedFiles = files.map((file) => path.parse(file));
  const prefix = `${srcdir}/`;
  const strip = (p: string) => (p.startsWith(prefix) ? p.slice(prefix.length) : p);

  const diffVersionedSpecNames = parsedFiles
    .filter(({ base }) => base === "index.ts")
    .map(({ dir }) => strip(dir));
  diffVersionedSpecNames.sort();

  const specNames = parsedFiles
    .filter(({ base, ext }) => base !== "index.ts" && ext === ".ts")
    .map((p) => strip(`${p.dir}/${p.name}`))
    .concat(diffVersionedSpecNames);
  specNames.sort();

  const modules = parsedFiles.map((p) => strip(`${p.dir}/${p.name}`));

  await fs.mkdir(path.join(outdir, "dynamic"), { recursive: true });

  await fs.writeFile(
    path.join(outdir, "index.js"),
    `var e=${JSON.stringify(specNames)},diffVersionedCompletions=${JSON.stringify(
      diffVersionedSpecNames,
    )};export{e as default,diffVersionedCompletions};`,
  );
  await fs.writeFile(
    path.join(outdir, "index.json"),
    JSON.stringify({
      completions: specNames,
      diffVersionedCompletions: diffVersionedSpecNames,
    }),
  );
  await fs.writeFile(
    path.join(outdir, "index.d.ts"),
    `declare const completions: string[]
declare const diffVersionedCompletions: string[]
export { completions as default, diffVersionedCompletions }
`,
  );
  await fs.writeFile(
    path.join(outdir, "dynamic/index.js"),
    `var e={${modules
      .map((mod) => `${JSON.stringify(mod)}:()=>import(${JSON.stringify(`../${mod}.js`)})`)
      .join(",")}};export{e as default};`,
  );
  await fs.writeFile(
    path.join(outdir, "dynamic/index.d.ts"),
    `declare const completions: {
    [key: string]: () => Promise<{
        default: any;
    }>
}
export { completions as default }
`,
  );
}

export interface CompileOptions {
  /** Directory the spec sources live in, relative to `cwd`. */
  srcdir?: string;
  /** Directory to write the compiled specs to, relative to `cwd`. */
  outdir: string;
}

export async function compileSpecs({
  srcdir = SOURCE_FOLDER_NAME,
  outdir,
}: CompileOptions): Promise<{ count: number; stubbed: string[] }> {
  const files: string[] = [];
  for await (const file of fs.glob(`${srcdir}/**/*.ts`)) {
    files.push(file);
  }
  // Upstream took whatever order fast-glob walked the tree in, which made
  // `dynamic/index.js` depend on the filesystem. Sorting makes the build
  // reproducible; every other output was already sorted.
  files.sort();

  if (files.length === 0) {
    throw new Error(`no spec sources matched ${srcdir}/**/*.ts in ${process.cwd()}`);
  }

  const stubbed = new Set<string>();
  await Promise.all([
    build({
      entryPoints: files,
      outdir,
      bundle: true,
      outbase: srcdir,
      format: "esm",
      minify: true,
      plugins: [nodeBuiltinStubs(stubbed)],
    }),
    generateIndex(outdir, srcdir, files),
  ]);

  return { count: files.length, stubbed: [...stubbed].sort() };
}
