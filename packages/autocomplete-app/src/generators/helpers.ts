import type { Timing } from "@fig/shared/timing";
import type { Annotation } from "@fig/autocomplete-parser/core";
import type { Suggestion } from "@fig/shared/internal";
import { getCWDForFilesAndFolders } from "@fig/shared/utils";
import { SETTINGS, type SettingsReader } from "@fig/shared/settings";
import { Cache } from "./cache";

export type GeneratorContext = Fig.ShellContext & {
  sessionId?: string;
  inputRevision?: bigint;
  annotations: Annotation[];
  tokenArray: string[];
  isDangerous?: boolean;
  searchTerm: string;
};

export type GeneratorState = {
  generator: Fig.Generator;
  context: GeneratorContext;
  loading: boolean;
  result: Suggestion[];
  request?: Promise<Fig.Suggestion[]>;
};

export const haveContextForGenerator = (context: GeneratorContext): boolean =>
  Boolean(context.currentWorkingDirectory);

export function createGeneratorCache(read: SettingsReader, now: () => number) {
  const generatorCache = new Cache(now);
  const generatorIds = new WeakMap<Fig.Generator, number>();
  let nextGeneratorId = 0;
  function generatorId(generator: Fig.Generator) {
    let id = generatorIds.get(generator);
    if (id === undefined) {
      id = ++nextGeneratorId;
      generatorIds.set(generator, id);
    }
    return id;
  }

  async function runCachedGenerator<T>(
    generator: Fig.Generator,
    context: GeneratorContext,
    initialRun: () => Promise<T>,
    cacheKey?: string /* This is generator.script or generator.script(...) */,
    signal?: AbortSignal,
    onRefresh?: (value: T) => void,
  ): Promise<T> {
    const cacheDefault = read<boolean>(SETTINGS.CACHE_ALL_GENERATORS) ?? false;
    let { cache } = generator;
    if (!cache && cacheDefault) {
      cache = { strategy: "stale-while-revalidate", ttl: 1_000 };
    }
    if (!cache) {
      return initialRun();
    }
    const { tokenArray, currentWorkingDirectory, searchTerm } = context;

    const directory = generator.template
      ? getCWDForFilesAndFolders(currentWorkingDirectory, searchTerm)
      : currentWorkingDirectory;

    // we cache generator results by script, if no script was provided we use the tokens instead
    const key = JSON.stringify([
      generatorId(generator),
      cache.cacheByDirectory ? directory : undefined,
      cacheKey ?? tokenArray,
      context.sessionId,
      context.environmentVariables,
    ]);

    return generatorCache.entry(key, initialRun, cache, signal, onRefresh);
  }

  return {
    runCachedGenerator,
    clear: () => generatorCache.clear(),
    get size() {
      return generatorCache.size;
    },
  };
}
export type GeneratorServices = {
  delay: Timing["delay"];
  listDirectory?(path: string, signal?: AbortSignal): Promise<string[]>;
  invoke<T>(fn: () => T): T;
  execute(
    input: Fig.ExecuteCommandInput,
    timeout?: number,
    signal?: AbortSignal,
  ): Promise<Fig.ExecuteCommandOutput>;
  cached: ReturnType<typeof createGeneratorCache>["runCachedGenerator"];
  history(annotations: Annotation[], currentProcess: string): Fig.TemplateSuggestion[];
  timeout(): number;
};
