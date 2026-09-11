#!/usr/bin/env node
import { parseArgs } from "node:util";
import { compileSpecs } from "./index.ts";

const { values } = parseArgs({
  options: {
    outdir: { type: "string", short: "o", default: "dist" },
    srcdir: { type: "string", short: "s", default: "src" },
    help: { type: "boolean", short: "h", default: false },
  },
});

if (values.help) {
  console.log(
    "usage: fig-spec-compile [-s|--srcdir <dir>] [-o|--outdir <dir>]\n\n" +
      "Bundles every <srcdir>/**/*.ts completion spec into one minified ESM\n" +
      "module under <outdir>, plus index.js / index.json / dynamic/index.js.",
  );
  process.exit(0);
}

const started = performance.now();
const { count, stubbed } = await compileSpecs({
  srcdir: values.srcdir,
  outdir: values.outdir,
});
const elapsed = Math.round(performance.now() - started);

if (stubbed.length > 0) {
  console.log(`stubbed Node builtins: ${stubbed.join(", ")}`);
}
console.log(`built ${count} specs into ${values.outdir} in ${elapsed}ms`);
