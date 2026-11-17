import { expect, it } from "vitest";
import { Cache } from "./cache";
const options: Fig.Cache = { strategy: "max-age", ttl: 10000 };
it("allows retry after a failed fetch", async () => {
  const cache = new Cache(() => Date.now());
  await expect(
    cache.entry("key", () => Promise.reject(new Error("failed")), options),
  ).rejects.toThrow("failed");
  expect(await cache.entry("key", () => Promise.resolve("fresh"), options)).toBe("fresh");
});
it("does not share or overwrite a new generation with cancelled in-flight work", async () => {
  const cache = new Cache(() => Date.now());
  const old = new AbortController();
  const current = new AbortController();
  let finish!: (value: string) => void;
  const first = cache.entry(
    "key",
    () =>
      new Promise<string>((resolve) => {
        finish = resolve;
      }),
    options,
    old.signal,
  );
  old.abort();
  expect(await cache.entry("key", () => Promise.resolve("new"), options, current.signal)).toBe(
    "new",
  );
  finish("old");
  await first;
  expect(cache.currentValue("key", options)).toBe("new");
  expect(
    await cache.entry(
      "key",
      () => Promise.resolve("unnecessary"),
      options,
      new AbortController().signal,
    ),
  ).toBe("new");
});
it("serves stale immediately and publishes refresh only in the owning live scope", async () => {
  const cache = new Cache(() => Date.now());
  const strategy: Fig.Cache = { strategy: "stale-while-revalidate", ttl: -1 };
  await cache.entry("key", async () => "old", strategy);
  let finish!: (value: string) => void;
  const controller = new AbortController();
  const values: string[] = [];
  const result = await cache.entry(
    "key",
    () =>
      new Promise<string>((resolve) => {
        finish = resolve;
      }),
    strategy,
    controller.signal,
    (value) => values.push(value),
  );
  expect(result).toBe("old");
  expect(values).toEqual([]);
  finish("fresh");
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(values).toEqual(["fresh"]);
  const stale = await cache.entry(
    "key",
    () =>
      new Promise<string>((resolve) => {
        finish = resolve;
      }),
    strategy,
    controller.signal,
    (value) => values.push(value),
  );
  expect(stale).toBe("fresh");
  controller.abort();
  finish("cancelled");
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(values).toEqual(["fresh"]);
  expect(cache.currentValue("key", strategy)).toBe("fresh");
});
