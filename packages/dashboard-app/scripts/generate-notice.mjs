#!/usr/bin/env node
/**
 * Regenerates the npm half of `public/license/NOTICE.txt`, the file the
 * dashboard's /licenses page fetches and prints verbatim.
 *
 *     node packages/dashboard-app/scripts/generate-notice.mjs [--check]
 *
 * The file has two halves. The first is the Rust dependency tree, which arrived
 * with the upstream source release and is not reproducible from here — it is
 * copied through untouched, up to and including NPM_HEADER below. Everything
 * after that marker is rewritten from the production dependency closure of the
 * three workspace packages whose build output ships inside the app bundle.
 *
 * Why those three: `q bundle` copies `packages/autocomplete-app/dist`,
 * `packages/dashboard-app/dist` and `specs/dist` into Resources. The specs are
 * compiled with esbuild, so a spec that imports `yaml` or `semver` inlines that
 * library into the shipped module even though no app package depends on it.
 *
 * `pnpm licenses list` is not the source here: it stops at a workspace link, so
 * `semver` (a dependency of `@fig/autocomplete-parser`) never appears in it.
 * `pnpm list --depth Infinity` walks through workspace packages and reports the
 * store path of each resolution, which is what the license text is read from.
 *
 * Workspace packages themselves are excluded: their licensing is the repo's own
 * LICENSE.MIT / LICENSE.APACHE and the vendoring table in NOTICE.
 */

import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

const REPO_ROOT = path.resolve(import.meta.dirname, "../../..");
const NOTICE_PATH = path.join(REPO_ROOT, "packages/dashboard-app/public/license/NOTICE.txt");

/** Workspace packages whose build output is copied into the app bundle. */
const SHIPPED = ["./packages/autocomplete-app", "./packages/dashboard-app", "./specs"];

const NPM_HEADER = `----

The npm packages below are bundled into the autocomplete app, the dashboard app
and the compiled completion specs. Regenerate this section with:

    node packages/dashboard-app/scripts/generate-notice.mjs

----
`;

const LICENSE_FILE = /^(LICEN[CS]E|COPYING)($|[.-])/i;

/** Every production dependency reachable from SHIPPED, workspace links included. */
function collectPackages() {
  const filters = SHIPPED.flatMap((pkg) => ["--filter", pkg]);
  const raw = execFileSync(
    "pnpm",
    ["list", "--prod", "--depth", "Infinity", "--json", ...filters],
    { cwd: REPO_ROOT, encoding: "utf8", maxBuffer: 256 * 1024 * 1024 },
  );

  const found = new Map();
  const visit = (deps) => {
    for (const [name, info] of Object.entries(deps ?? {})) {
      if (!info?.path || !info.version) continue;
      // A workspace package resolves to its own directory, not into the store.
      if (!info.path.includes(`${path.sep}node_modules${path.sep}`)) continue;
      const key = `${name}@${info.version}`;
      if (!found.has(key)) {
        found.set(key, { name, version: info.version, dir: info.path });
        visit(info.dependencies);
      }
    }
  };
  for (const project of JSON.parse(raw)) {
    visit(project.dependencies);
    visit(project.optionalDependencies);
  }
  return [...found.values()].sort((a, b) =>
    a.name === b.name ? a.version.localeCompare(b.version) : a.name.localeCompare(b.name),
  );
}

/**
 * The package's own license text, or — for the handful that ship none — the
 * SPDX expression its package.json declares, so the entry is still accounted
 * for instead of silently dropped.
 */
function licenseText({ name, dir }) {
  const file = fs.readdirSync(dir).find((entry) => LICENSE_FILE.test(entry));
  if (file) {
    const stat = fs.statSync(path.join(dir, file));
    if (stat.isFile()) return fs.readFileSync(path.join(dir, file), "utf8").trim();
  }
  const manifest = JSON.parse(fs.readFileSync(path.join(dir, "package.json"), "utf8"));
  const spdx = typeof manifest.license === "string" ? manifest.license : manifest.license?.type;
  if (!spdx) throw new Error(`${name} declares no license and ships no license file`);
  return `This package ships no license file. Its package.json declares: ${spdx}`;
}

function render(packages) {
  return packages
    .map(
      (pkg) =>
        `\nPackage: ${pkg.name.replaceAll("/", ":")}:${pkg.version}\n\n` +
        `The following copyrights and licenses were found in the source code of this package:\n\n` +
        `${licenseText(pkg)}\n\n----\n`,
    )
    .join("");
}

const existing = fs.readFileSync(NOTICE_PATH, "utf8");
const markerAt = existing.indexOf(NPM_HEADER);
if (markerAt === -1) {
  throw new Error(
    `${NOTICE_PATH} has no npm section marker. Insert NPM_HEADER where the Rust ` +
      `crate entries end, then re-run.`,
  );
}

const packages = collectPackages();
const next = existing.slice(0, markerAt) + NPM_HEADER + render(packages);

if (process.argv.includes("--check")) {
  if (next !== existing) {
    console.error(
      "NOTICE.txt is out of date. Run: node packages/dashboard-app/scripts/generate-notice.mjs",
    );
    process.exit(1);
  }
  console.log(`NOTICE.txt is up to date (${packages.length} npm packages).`);
} else {
  fs.writeFileSync(NOTICE_PATH, next);
  console.log(`wrote ${packages.length} npm packages to ${path.relative(REPO_ROOT, NOTICE_PATH)}`);
}
