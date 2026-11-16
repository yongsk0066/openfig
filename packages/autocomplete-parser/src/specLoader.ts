import logger, { type Logger } from "loglevel";
import * as semver from "semver";
import { LruCache } from "@fig/shared/lru";
import { ensureTrailingSlash, withTimeout } from "@fig/shared/utils";
import { MOST_USED_SPECS } from "./constants.js";
import { LoadLocalSpecError } from "./errors.js";
function parseSpecIndex(value: unknown): {
  completions: string[];
  diffVersionedCompletions: string[];
} {
  if (!value || typeof value !== "object") throw new Error("Invalid spec index");
  const { completions, diffVersionedCompletions } = value as Record<string, unknown>;
  if (
    !Array.isArray(completions) ||
    !completions.every((name) => typeof name === "string") ||
    !Array.isArray(diffVersionedCompletions) ||
    !diffVersionedCompletions.every((name) => typeof name === "string")
  ) {
    throw new Error("Invalid spec index entries");
  }
  return { completions, diffVersionedCompletions };
}

export type SpecFileImport =
  | {
      default: Fig.Spec;
      getVersionCommand?: Fig.GetVersionCommand;
    }
  | {
      default: Fig.Subcommand;
      versions: Fig.VersionDiffMap;
    };

export type SpecLoaderPorts = {
  executeCommand: Fig.ExecuteCommandFunction;
  /** Shared version requests must not inherit one input subscriber's abort signal. */
  executeVersionCommand?: Fig.ExecuteCommandFunction;
  fread(path: string): Promise<string>;
  isInDevMode(): boolean;
  importSource(source: string): Promise<SpecFileImport>;
  importModule(url: string): Promise<SpecFileImport>;
  readJson(url: string): Promise<unknown>;
};
export function createSpecCatalogCaches() {
  return {
    stringImportCache: new LruCache<string, SpecFileImport>(64),
    publicSpecsRequest: undefined as
      | Promise<{ completions: Set<string>; diffVersionedSpecs: Set<string> }>
      | undefined,
  };
}
export function createSpecLoaderCaches(catalog = createSpecCatalogCaches()) {
  return {
    catalog,
    cachedCLIVersions: new LruCache<string, string>(128),
    versionRequests: new LruCache<string, Promise<string | undefined>>(128),
  };
}
export function createSpecLoader(
  {
    executeCommand,
    executeVersionCommand = executeCommand,
    fread,
    isInDevMode,
    importModule,
    importSource,
    readJson,
  }: SpecLoaderPorts,
  caches = createSpecLoaderCaches(),
) {
  const { cachedCLIVersions, versionRequests, catalog } = caches;
  const { stringImportCache } = catalog;
  // Keyed by the spec source text, so every edit of a local spec adds an entry.

  const importString = async (str: string) => {
    if (stringImportCache.has(str)) {
      return stringImportCache.get(str)!;
    }
    const result = await importSource(str);
    stringImportCache.set(str, result);
    return result;
  };

  /*
   * Deprecated: eventually will just use importLocalSpec above
   * Load a spec import("{path}/{name}")
   */
  async function importSpecFromFile(
    name: string,
    path: string,
    localLogger: Logger = logger,
  ): Promise<SpecFileImport> {
    const importFromPath = async (fullPath: string) => {
      localLogger.info(`Loading spec from ${fullPath}`);
      const contents = await fread(fullPath);
      if (!contents) {
        throw new LoadLocalSpecError(`Failed to read file: ${fullPath}`);
      }
      return contents;
    };

    let result: string;
    const joinedPath = `${ensureTrailingSlash(path)}${name}`;
    try {
      result = await importFromPath(`${joinedPath}.js`);
    } catch (_) {
      result = await importFromPath(`${joinedPath}/index.js`);
    }

    return importString(result);
  }

  // TODO: this is a problem for diff-versioned specs
  async function importFromPublicCDN<T = SpecFileImport>(name: string): Promise<T> {
    return withTimeout(20000, importModule(`spec://localhost/${name}.js`) as Promise<T>);
  }

  async function jsonFromPublicCDN(path: string): Promise<unknown> {
    return readJson(`spec://localhost/${path}.json`);
  }

  // TODO: this is a problem for diff-versioned specs
  async function importFromLocalhost<T = SpecFileImport>(
    name: string,
    port: number | string,
  ): Promise<T> {
    return withTimeout(20000, importModule(`http://localhost:${port}/${name}.js`) as Promise<T>);
  }

  const getCachedCLIVersion = (key: string) => cachedCLIVersions.get(key) ?? null;

  async function getVersionFromFullFile(specData: SpecFileImport, name: string) {
    if (typeof specData.default !== "function") return undefined;
    const storageKey = `cliVersion-${name}`;
    const dev = isInDevMode();
    if (!dev) {
      const version = getCachedCLIVersion(storageKey);
      if (version !== null) return version;
      const pending = versionRequests.get(storageKey);
      if (pending) return pending;
    }
    const request = (async () => {
      try {
        const version =
          "getVersionCommand" in specData && specData.getVersionCommand
            ? await specData.getVersionCommand(executeVersionCommand)
            : (semver.clean(
                (await executeVersionCommand({ command: name, args: ["--version"] })).stdout,
              ) ?? undefined);
        if (version && !dev) cachedCLIVersions.set(storageKey, version);
        return version;
      } catch {
        // A failed query is retryable; never persist a negative result.
        return undefined;
      }
    })();
    if (!dev) versionRequests.set(storageKey, request);
    try {
      return await request;
    } finally {
      if (versionRequests.get(storageKey) === request) versionRequests.delete(storageKey);
    }
  }

  // TODO: cache this request using SWR strategy
  function clearSpecIndex() {
    catalog.publicSpecsRequest = undefined;
  }

  const createPublicSpecsRequest = async () => {
    if (catalog.publicSpecsRequest === undefined) {
      catalog.publicSpecsRequest = jsonFromPublicCDN("index")
        .then(parseSpecIndex)
        .then((index) => ({
          completions: new Set(index.completions),
          diffVersionedSpecs: new Set(index.diffVersionedCompletions),
        }))
        .catch(() => {
          catalog.publicSpecsRequest = undefined;
          return { completions: new Set(), diffVersionedSpecs: new Set() };
        });
    }
    return catalog.publicSpecsRequest;
  };

  async function publicSpecExists(name: string): Promise<boolean> {
    const { completions } = await createPublicSpecsRequest();
    return completions.has(name);
  }

  async function isDiffVersionedSpec(name: string): Promise<boolean> {
    const { diffVersionedSpecs } = await createPublicSpecsRequest();
    return diffVersionedSpecs.has(name);
  }

  async function preloadSpecs(): Promise<SpecFileImport[]> {
    return Promise.all(
      MOST_USED_SPECS.map(async (name) => {
        // TODO: refactor everything to allow the correct diff-versioned specs to be loaded
        // too, now we are only loading the index
        if (await isDiffVersionedSpec(name)) {
          return importFromPublicCDN(`${name}/index`);
        }
        return importFromPublicCDN(name);
      }).map((promise) => promise.catch((e) => e)),
    );
  }

  return {
    importString,
    importSpecFromFile,
    importFromPublicCDN,
    importFromLocalhost,
    getCachedCLIVersion,
    getVersionFromFullFile,
    clearSpecIndex,
    parseSpecIndex,
    publicSpecExists,
    isDiffVersionedSpec,
    preloadSpecs,
    clear() {
      stringImportCache.clear();
      catalog.publicSpecsRequest = undefined;
      cachedCLIVersions.clear();
      versionRequests.clear();
    },
  };
}
