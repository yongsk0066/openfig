import { LruCache } from "@fig/shared/lru";
import { convertSubcommand, initializeDefault } from "@fig/spec-helpers";
import { getCommand } from "@fig/shell-parser";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createCache, generateSpecCache, resetCaches, specCache } from "../src/caches";
import { loadSubcommandCached } from "../src/loadSpec";
import { clearFigCaches, parseArguments } from "../src/parseArguments";

vi.mock("../src/loadSpec", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../src/loadSpec")>()),
  loadSubcommandCached: vi.fn(),
}));

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

const generateSpec = vi.fn(async () => ({ name: "fig", subcommands: [{ name: "doctor" }] }));
const fig = convertSubcommand({ name: "fig", generateSpec }, initializeDefault);

/** `fig ` typed in `cwd`: a generated-spec parse, cached under a key per directory. */
const parseFigIn = (cwd: string) =>
  parseArguments(getCommand("fig ", {}), {
    currentWorkingDirectory: cwd,
    currentProcess: "zsh",
    sshPrefix: "",
    environmentVariables: {},
  });

/** How many parse-cache entries `clearFigCaches` still thinks it has to drop. */
const indexedKeys = () => {
  const deletes = vi.spyOn(LruCache.prototype, "delete");
  clearFigCaches();
  const count = deletes.mock.calls.length;
  deletes.mockRestore();
  return count;
};

describe("fig command parse cache index (figCaches)", () => {
  beforeEach(() => {
    globalThis.fig = { ...globalThis.fig, settings: {} };
    vi.mocked(loadSubcommandCached).mockResolvedValue(fig);
    resetCaches();
    clearFigCaches();
    generateSpec.mockClear();
  });

  afterEach(() => {
    vi.mocked(loadSubcommandCached).mockReset();
  });

  it("clearFigCaches drops the cached `fig` parses, once", async () => {
    await parseFigIn("/repo");
    await parseFigIn("/repo");
    expect(generateSpec).toHaveBeenCalledTimes(1);

    expect(indexedKeys()).toBe(1);
    await parseFigIn("/repo");
    expect(generateSpec).toHaveBeenCalledTimes(2);

    // The index was emptied along with the entries it pointed at.
    clearFigCaches();
    expect(indexedKeys()).toBe(0);
  });

  it("indexes no more parses than the cache it points into holds", async () => {
    for (let i = 0; i < 1_200; i += 1) {
      await parseFigIn(`/repo-${i}`);
    }
    expect(indexedKeys()).toBeLessThanOrEqual(1_000);
  });

  it("resetCaches forgets the index with the parse caches", async () => {
    await parseFigIn("/a");
    await parseFigIn("/b");
    resetCaches();
    expect(indexedKeys()).toBe(0);
  });
});
