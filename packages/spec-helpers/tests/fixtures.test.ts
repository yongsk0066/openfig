import fs from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { getVersionFromVersionedSpec } from "../src/versions";

/**
 * Upstream ran this as a standalone `tsx test/index.ts` script that wrote an
 * `output.json` next to each fixture and string-compared it against a
 * prettier-formatted `expected.json`. Same fixtures, same assertion, but as a
 * vitest case comparing parsed values, so formatting is not part of the test.
 */
const fixturesPath = path.join(import.meta.dirname, "fixtures");

describe("getVersionFromVersionedSpec fixtures", () => {
  for (const entry of fs.readdirSync(fixturesPath, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue;

    it(entry.name, async () => {
      const dir = path.join(fixturesPath, entry.name);
      const { targetVersion } = JSON.parse(fs.readFileSync(path.join(dir, "config.json"), "utf8"));
      const expected = JSON.parse(fs.readFileSync(path.join(dir, "expected.json"), "utf8"));

      const { completion, versions } = await import(path.join(dir, "spec", "1.0.0.js"));

      expect(getVersionFromVersionedSpec(completion, versions, targetVersion)).toEqual(expected);
    });
  }
});
