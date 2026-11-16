import { readSettings } from "@fig/shared/settings";
import { sleep } from "@fig/shared/utils";
import type { Annotation } from "@fig/autocomplete-parser";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createGeneratorCache, haveContextForGenerator, type GeneratorContext } from "../helpers";

const context: GeneratorContext = {
  annotations: [] as Annotation[],
  tokenArray: [] as string[],
  currentWorkingDirectory: "",
  currentProcess: "",
  sshPrefix: "",
  searchTerm: "",
  environmentVariables: {},
};

describe("haveContextForGenerator", () => {
  it("true with context", () => {
    expect(
      haveContextForGenerator({
        ...context,
        currentWorkingDirectory: "/",
        currentProcess: "zsh",
      }),
    ).toBe(true);
  });

  it("true with context in ssh", () => {
    expect(
      haveContextForGenerator({
        ...context,
        currentWorkingDirectory: "/",
        currentProcess: "ssh",
      }),
    ).toBe(true);
  });

  it("false when no current dir", () => {
    expect(
      haveContextForGenerator({
        ...context,
        currentWorkingDirectory: "",
      }),
    ).toBe(false);
  });
});

describe("runCachedGenerator", () => {
  describe("no cache object", () => {
    const initialRun = vi.fn();

    beforeEach(() => {
      generatorCache.clear();
    });

    afterEach(() => {
      initialRun.mockClear();
    });

    it("should return the right result", async () => {
      initialRun.mockResolvedValue("result");

      expect(await runCachedGenerator({}, context, initialRun, "command")).toEqual("result");
      expect(initialRun).toHaveBeenCalled();
    });

    it("should call initialRun many times without cache", async () => {
      for (let i = 0; i < 2; i += 1) {
        await runCachedGenerator({}, context, initialRun, "command");
      }

      expect(initialRun).toHaveBeenCalledTimes(2);
    });
  });

  describe("should respect cacheByDirectory key when cache is specified", () => {
    const initialRun = vi.fn();

    beforeEach(() => {
      generatorCache.clear();
    });

    afterEach(() => {
      initialRun.mockClear();
    });

    it("should call initialRun once with cache by directory not expired", async () => {
      const generator: Fig.Generator = {
        cache: {
          ttl: 1000,
          strategy: "max-age",
          cacheByDirectory: true,
        },
      };

      for (let i = 0; i < 2; i += 1) {
        await runCachedGenerator(generator, context, initialRun, "command");
      }

      expect(initialRun).toHaveBeenCalledTimes(1);
    });

    it("should call initialRun twice with directory change", async () => {
      const contexts = [context, { ...context, currentWorkingDirectory: "/test" }];
      const generator: Fig.Generator = {
        cache: {
          ttl: 1000,
          strategy: "max-age",
          cacheByDirectory: true,
        },
      };

      for (let i = 0; i < 2; i += 1) {
        await runCachedGenerator(generator, contexts[i], initialRun, "command");
      }

      expect(initialRun).toHaveBeenCalledTimes(2);
    });

    it("should call initialRun once with directory change and not caching by directory", async () => {
      const contexts = [context, { ...context, currentWorkingDirectory: "/test" }];
      const generator: Fig.Generator = {
        cache: {
          ttl: 1000,
          strategy: "max-age",
          cacheByDirectory: false,
        },
      };

      for (let i = 0; i < 2; i += 1) {
        await runCachedGenerator(generator, contexts[i], initialRun, "command");
      }

      expect(initialRun).toHaveBeenCalledTimes(1);
    });
  });

  describe("should attempt to cache based on `script` field", () => {
    const initialRun = vi.fn();

    beforeEach(() => {
      generatorCache.clear();
    });

    afterEach(() => {
      initialRun.mockClear();
    });

    it("caches generator based on script, not tokens, if script field is static", async () => {
      const contexts = [context, { ...context, tokenArray: ["a", "b"] }];
      const generator: Fig.Generator = {
        script: ["echo", "hello world"],
        cache: {
          strategy: "max-age",
          ttl: 1000,
        },
      };

      const generators: Fig.Generator[] = [generator, generator];

      for (let i = 0; i < 2; i += 1) {
        await runCachedGenerator(
          generators[i],
          contexts[i],
          initialRun,
          JSON.stringify(generator.script),
        );
      }

      expect(initialRun).toHaveBeenCalledTimes(1);
    });

    it("caches generator based on tokens, if script field is a FUNCTION", async () => {
      const contexts = [context, { ...context, tokenArray: ["a", "b"] }];
      const generator: Fig.Generator = {
        script: ["echo", "hello world"],
        cache: {
          strategy: "max-age",
          ttl: 1000,
        },
      };

      const generators: Fig.Generator[] = [generator, generator];

      for (let i = 0; i < 2; i += 1) {
        await runCachedGenerator(
          generators[i],
          contexts[i],
          initialRun,
          JSON.stringify(generator.script),
        );
      }

      expect(initialRun).toHaveBeenCalledTimes(1);
    });

    // todo: implement a better approach to caching custom generators
    it("caches generator based on tokens, if using CUSTOM generator", async () => {
      const contexts = [context, { ...context, tokenArray: ["a", "b"] }];
      const generator: Fig.Generator = {
        custom: async () => [],
        cache: {
          strategy: "max-age",
          ttl: 1000,
        },
      };

      const generators: Fig.Generator[] = [generator, generator];

      for (let i = 0; i < 2; i += 1) {
        await runCachedGenerator(generators[i], contexts[i], initialRun);
      }

      expect(initialRun).toHaveBeenCalledTimes(2);
    });
  });

  describe("no cache strategy specified (stale-while-revalidate)", () => {
    const generator: Fig.Generator = { cache: { ttl: 1_000 } };

    beforeEach(() => {
      generatorCache.clear();
    });

    it("should return the right result", async () => {
      expect(
        await runCachedGenerator(
          generator,
          context,
          () => Promise.resolve("stale-data"),
          "command",
        ),
      ).toEqual("stale-data");
      expect(
        await runCachedGenerator(
          generator,
          context,
          () => Promise.resolve("fresh-data"),
          "command",
        ),
      ).toEqual("stale-data");
      await sleep(1_500);
      expect(
        await runCachedGenerator(
          generator,
          context,
          () => Promise.resolve("fresher-data"),
          "command",
        ),
      ).toEqual("stale-data");
    });
  });

  describe("max-age cache strategy", () => {
    const generator: Fig.Generator = {
      cache: { strategy: "max-age", ttl: 1_000 },
    };

    beforeEach(() => {
      generatorCache.clear();
    });

    it("should return the right result", async () => {
      expect(
        await runCachedGenerator(
          generator,
          context,
          () => Promise.resolve("stale-data"),
          "command",
        ),
      ).toEqual("stale-data");
      expect(
        await runCachedGenerator(
          generator,
          context,
          () => Promise.resolve("fresh-data"),
          "command",
        ),
      ).toEqual("stale-data");
      await sleep(1_500);
      expect(
        await runCachedGenerator(
          generator,
          context,
          () => Promise.resolve("fresher-data"),
          "command",
        ),
      ).toEqual("fresher-data");
    });
  });

  describe("stale-while-revalidate cache strategy", () => {
    const generator: Fig.Generator = {
      cache: { strategy: "stale-while-revalidate", ttl: 1_000 },
    };

    beforeEach(() => {
      generatorCache.clear();
    });

    it("should return the right result", async () => {
      expect(
        await runCachedGenerator(
          generator,
          context,
          () => Promise.resolve("stale-data"),
          "command",
        ),
      ).toEqual("stale-data");
      await sleep(1_500);
      expect(
        await runCachedGenerator(
          generator,
          context,
          () => Promise.resolve("fresh-data"),
          "command",
        ),
      ).toEqual("stale-data");
      await vi.waitFor(() =>
        expect(
          generatorCache.runCachedGenerator(
            generator,
            context,
            () => Promise.resolve("fresh-data"),
            "command",
          ),
        ).resolves.toBe("fresh-data"),
      );
      expect(
        await runCachedGenerator(
          generator,
          context,
          () => Promise.resolve("fresh-data"),
          "command",
        ),
      ).toEqual("fresh-data");
    });
  });
});

it("isolates cached generators and terminal sessions even with identical tokens", async () => {
  generatorCache.clear();
  const a: Fig.Generator = { cache: { ttl: 1000 } },
    b: Fig.Generator = { cache: { ttl: 1000 } };
  const first = vi.fn(async () => "first"),
    second = vi.fn(async () => "second");
  expect(await runCachedGenerator(a, context, first)).toBe("first");
  expect(await runCachedGenerator(b, context, second)).toBe("second");
  expect(await runCachedGenerator(a, { ...context, sessionId: "other" }, second)).toBe("second");
  expect(await runCachedGenerator(a, context, first)).toBe("first");
  expect(first).toHaveBeenCalledOnce();
});

const generatorCache = createGeneratorCache(readSettings({}));
const { runCachedGenerator } = generatorCache;
