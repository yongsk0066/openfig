import { describe, it, expect } from "vitest";
import { createParserCaches } from "../src/parserCore";
import { createSpecLoaderCaches } from "../src/specLoader";
describe("owned parser caches", () => {
  it("bounds each parse cache and keeps instances isolated", () => {
    const a = createParserCaches(),
      b = createParserCaches();
    for (let i = 0; i < 1500; i++) a.parseArgumentsCache.set(String(i), {} as never);
    expect(a.parseArgumentsCache.size).toBe(1000);
    expect(a.parseArgumentsCache.get("0")).toBeUndefined();
    expect(b.parseArgumentsCache.size).toBe(0);
  });
  it("bounds generated specs and module imports", () => {
    const parser = createParserCaches(),
      loader = createSpecLoaderCaches();
    for (let i = 0; i < 1500; i++) {
      parser.generateSpecCache.set(String(i), {} as never);
      loader.catalog.stringImportCache.set(String(i), {} as never);
    }
    expect(parser.generateSpecCache.size).toBeLessThanOrEqual(256);
    expect(loader.catalog.stringImportCache.size).toBeLessThanOrEqual(64);
  });
});
