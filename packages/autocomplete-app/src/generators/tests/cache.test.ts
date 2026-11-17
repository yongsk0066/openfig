import { describe, expect, it } from "vitest";
import { Cache } from "../cache";

describe("generator Cache", () => {
  it("holds at most 500 entries however many keys it sees", async () => {
    const cache = new Cache(() => Date.now());
    const strategy: Fig.Cache = { strategy: "max-age", ttl: 60_000 };
    for (let i = 0; i < 2_000; i += 1) {
      await cache.entry(`/dir-${i},ls -1`, async () => [`file-${i}`], strategy);
      expect(cache.size).toBeLessThanOrEqual(500);
    }
    // Recent entries are still served from the cache.
    let ran = false;
    const value = await cache.entry(
      "/dir-1999,ls -1",
      async () => {
        ran = true;
        return [];
      },
      strategy,
    );
    expect(ran).toBe(false);
    expect(value).toEqual(["file-1999"]);
  });
});
