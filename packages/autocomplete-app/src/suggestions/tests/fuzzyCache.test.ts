import fuzzysort from "@fig/fuzzysort";
import { afterEach, describe, expect, it } from "vitest";
import { getFuzzyNameMatches } from "../helpers";

// Folder names a `cd` in a large tree would offer, one batch per keystroke.
const batch = (keystroke: number) =>
  Array.from({ length: 5 }, (_, i) => `project-${keystroke}-${i}/`);

describe("fuzzysort prepared caches", () => {
  afterEach(() => fuzzysort.cleanup());

  it("stay bounded over 10,000 keystrokes of distinct candidates", () => {
    let maxPrepared = 0;
    let maxSearches = 0;
    for (let keystroke = 0; keystroke < 10_000; keystroke += 1) {
      getFuzzyNameMatches(batch(keystroke), `pro${keystroke}`);
      const sizes = fuzzysort.cacheSizes();
      maxPrepared = Math.max(maxPrepared, sizes.prepared);
      maxSearches = Math.max(maxSearches, sizes.preparedSearch);
    }
    // 50,000 distinct targets and 10,000 distinct queries went through.
    expect(maxPrepared).toBeLessThanOrEqual(10_000);
    expect(maxSearches).toBeLessThanOrEqual(1_000);
  });

  it("scores an evicted target the same as a cached one", () => {
    const first = getFuzzyNameMatches(["checkout", "cherry-pick"], "chk").map((r) => r?.score);
    for (let i = 0; i < 20_000; i += 1) fuzzysort.single("x", `filler-${i}`);
    const again = getFuzzyNameMatches(["checkout", "cherry-pick"], "chk").map((r) => r?.score);
    expect(again).toEqual(first);
  });

  it("keeps a list that is re-matched on every keystroke prepared", () => {
    const names = Array.from({ length: 5_000 }, (_, i) => `file-${i}.txt`);
    getFuzzyNameMatches(names, "f");
    const warm = fuzzysort.cacheSizes().prepared;
    for (const query of ["fi", "fil", "file", "file-", "file-1"]) {
      getFuzzyNameMatches(names, query);
      expect(fuzzysort.cacheSizes().prepared).toBe(warm);
    }
    expect(warm).toBe(5_000);
  });

  it("cleanup empties both caches", () => {
    getFuzzyNameMatches(batch(0), "pro");
    expect(fuzzysort.cacheSizes().prepared).toBeGreaterThan(0);
    fuzzysort.cleanup();
    expect(fuzzysort.cacheSizes()).toEqual({ prepared: 0, preparedSearch: 0 });
  });
});
