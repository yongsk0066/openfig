import { describe, expect, it } from "vitest";
import { createCache, generateSpecCache, resetCaches, specCache } from "../src/caches";

describe("parser caches", () => {
  it("createCache defaults to 1,000 entries (the parseArguments caches)", () => {
    const cache = createCache<{ tokens: string }>();
    for (let i = 0; i < 50_000; i += 1) {
      cache.set(`git checkout branch-${i},git,/repo-${i % 97},zsh`, { tokens: `${i}` });
      if (cache.size > 1_000) throw new Error(`size ${cache.size} after ${i + 1} inserts`);
    }
    expect(cache.size).toBeLessThanOrEqual(1_000);
    expect(cache.get(`git checkout branch-49999,git,/repo-${49_999 % 97},zsh`)).toEqual({
      tokens: "49999",
    });
  });

  it("spec caches hold at most 256 converted specs", () => {
    for (let i = 0; i < 1_000; i += 1) {
      const spec = { name: [`spec-${i}`] } as never;
      specCache.set(`global,,spec-${i}`, spec);
      generateSpecCache.set(`generated-${i}`, spec);
    }
    expect(specCache.size).toBeLessThanOrEqual(256);
    expect(generateSpecCache.size).toBeLessThanOrEqual(256);
  });

  it("resetCaches empties every cache", () => {
    const cache = createCache<number>(10);
    cache.set("a", 1);
    specCache.set("b", { name: ["b"] } as never);
    resetCaches();
    expect(cache.size).toBe(0);
    expect(specCache.size).toBe(0);
  });
});
