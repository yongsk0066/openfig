import { describe, expect, it } from "vitest";
import { LruCache } from "../src/lru";

describe("LruCache", () => {
  it("never holds more than maxSize entries", () => {
    const cache = new LruCache<string, number>(100);
    let maxSeen = 0;
    for (let i = 0; i < 10_000; i += 1) {
      cache.set(`key-${i}`, i);
      maxSeen = Math.max(maxSeen, cache.size);
    }
    expect(maxSeen).toBeLessThanOrEqual(100);
    expect(cache.size).toBeLessThanOrEqual(100);
    expect([...cache.keys()]).toHaveLength(cache.size);
    // The most recent writes are still there, the first ones are gone.
    expect(cache.get("key-9999")).toBe(9999);
    expect(cache.get("key-9950")).toBe(9950);
    expect(cache.has("key-0")).toBe(false);
  });

  it("keeps entries that keep being read", () => {
    const cache = new LruCache<string, string>(10);
    cache.set("hot", "value");
    for (let i = 0; i < 1_000; i += 1) {
      cache.set(`cold-${i}`, "x");
      expect(cache.get("hot")).toBe("value");
      expect(cache.size).toBeLessThanOrEqual(10);
    }
  });

  it("an entry used within the last maxSize/2 writes survives", () => {
    const cache = new LruCache<number, number>(8);
    for (let i = 0; i < 4; i += 1) cache.set(i, i);
    cache.get(0);
    for (let i = 4; i < 7; i += 1) cache.set(i, i);
    expect(cache.get(0)).toBe(0);
  });

  it("stores falsy and undefined values", () => {
    const cache = new LruCache<string, unknown>(6);
    cache.set("zero", 0).set("undefined", undefined).set("a", 1).set("b", 2);
    // Both now sit in the old generation; reading them must promote, not lose, them.
    expect(cache.has("undefined")).toBe(true);
    expect(cache.get("undefined")).toBeUndefined();
    expect(cache.get("zero")).toBe(0);
    cache.set("c", 3).set("d", 4);
    expect(cache.has("zero")).toBe(true);
    expect(cache.has("undefined")).toBe(true);
  });

  it("overwrites, deletes and clears", () => {
    const cache = new LruCache<string, number>(4);
    cache.set("a", 1).set("b", 2).set("c", 3);
    cache.set("a", 10);
    expect(cache.get("a")).toBe(10);
    expect(cache.size).toBe(3);
    expect(cache.delete("b")).toBe(true);
    expect(cache.delete("b")).toBe(false);
    expect(cache.size).toBe(2);
    cache.clear();
    expect(cache.size).toBe(0);
    expect(cache.has("a")).toBe(false);
  });

  it("rejects a capacity it cannot honour", () => {
    expect(() => new LruCache(1)).toThrow(RangeError);
    expect(() => new LruCache(Number.POSITIVE_INFINITY)).toThrow(RangeError);
  });
});
