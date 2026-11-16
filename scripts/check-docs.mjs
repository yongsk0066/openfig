import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, statSync } from "node:fs";
import { dirname, resolve } from "node:path";

// Check repository-local file links and root-relative docs references in source comments.
// Remote URLs and heading anchors are intentionally outside this check.
const files = [
  ...new Set(
    execFileSync("git", ["ls-files", "-z", "--cached", "--others", "--exclude-standard"], {
      encoding: "utf8",
    }).split("\0"),
  ),
].filter((file) => file && existsSync(file) && statSync(file).isFile());
const errors = [];
let links = 0;
let documents = 0;
for (const file of files) {
  if (file.endsWith(".md")) {
    documents++;
    const source = readFileSync(file, "utf8").replace(/^```[^\n]*\n[\s\S]*?^```\s*$/gm, "");
    for (const [, target] of source.matchAll(/\[[^\]\n]*\]\(([^\s)]+)\)/g)) {
      if (/^(?:[a-z]+:|#|\/\/)/i.test(target)) continue;
      const path = decodeURIComponent(target.split("#")[0]);
      links++;
      if (!existsSync(resolve(dirname(file), path))) errors.push(`${file}: missing ${target}`);
    }
  } else if (/\.(?:rs|tsx?|js|mjs|sh|zsh)$/.test(file) || file === "Justfile") {
    const source = readFileSync(file, "utf8");
    for (const [path] of source.matchAll(/(?<![\w/.-])docs\/[\w-]+\.md\b/g)) {
      if (!existsSync(path)) errors.push(`${file}: missing ${path}`);
    }
  }
}
if (errors.length) throw new Error(errors.join("\n"));
console.log(`Documentation: ${documents} files, ${links} local links; source doc references valid`);
