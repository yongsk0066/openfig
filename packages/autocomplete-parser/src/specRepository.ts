import logger, { type Logger } from "loglevel";
import {
  convertSubcommand,
  initializeDefault,
  getVersionFromVersionedSpec,
} from "@fig/spec-helpers";
import { withTimeout, SpecLocationSource, splitPath, ensureTrailingSlash } from "@fig/shared/utils";
import { LruCache } from "@fig/shared/lru";
import type { Subcommand, SpecLocation } from "@fig/shared/internal";
import { SETTINGS, type SettingsReader } from "@fig/shared/settings";
import type { createSpecLoader, SpecFileImport } from "./specLoader";
import { DisabledSpecError, MissingSpecError, WrongDiffVersionedSpecError } from "./errors";
export const serializeSpecLocation = (location: SpecLocation): string => {
  if (location.type === SpecLocationSource.GLOBAL) {
    return `global://name=${location.name}`;
  }
  return `local://path=${location.path ?? ""}&name=${location.name}`;
};

export type SpecRepositoryPorts = {
  signal?: AbortSignal;
  loader: ReturnType<typeof createSpecLoader>;
  executeCommand: Fig.ExecuteCommandFunction;
  getSetting: SettingsReader;
  setSetting(key: string, value: unknown): Promise<unknown>;
  isInDevMode(): boolean;
};
export function createSpecRepository(
  { loader, executeCommand, getSetting, setSetting, isInDevMode, signal }: SpecRepositoryPorts,
  specCache = new LruCache<string, Subcommand>(256),
) {
  const {
    importBundledSpec,
    bundledSpecExists,
    importSpecFromFile,
    isDiffVersionedSpec,
    importFromLocalhost,
    getVersionFromFullFile,
  } = loader;

  /**
   * This searches for the first directory containing a .fig/ folder in the parent directories
   */
  const searchFigFolder = async (currentDirectory: string) => {
    try {
      return ensureTrailingSlash(
        (
          await executeCommand({
            command: "bash",
            args: [
              "-c",
              `until [[ -f .fig/autocomplete/build/_shortcuts.js ]] || [[ $PWD = $HOME ]] || [[ $PWD = "/" ]]; do cd ..; done; echo $PWD`,
            ],
            cwd: currentDirectory,
          })
        ).stdout,
      );
    } catch {
      return ensureTrailingSlash(currentDirectory);
    }
  };

  const getSpecPath = async (
    name: string,
    cwd: string,
    isScript?: boolean,
  ): Promise<SpecLocation> => {
    if (name === "?") {
      // If the user is searching for _shortcuts.js by using "?"
      const path = await searchFigFolder(cwd);
      return { name: "_shortcuts", type: SpecLocationSource.LOCAL, path };
    }

    const personalShortcutsToken = getSetting(SETTINGS.PERSONAL_SHORTCUTS_TOKEN) || "+";
    if (name === personalShortcutsToken) {
      return { name: "+", type: SpecLocationSource.LOCAL, path: "~/" };
    }

    const [path, basename] = splitPath(name);

    if (!isScript) {
      const type = SpecLocationSource.GLOBAL;

      // If `isScript` is undefined, we are parsing the first token, and
      // any path with a / is a script.
      if (isScript === undefined) {
        // special-case: Symfony has "bin/console" which can be invoked directly
        // and should not require a user to create script completions for it
        if (name === "bin/console" || name.endsWith("/bin/console")) {
          return { name: "php/bin-console", type };
        }
        if (!path.includes("/")) {
          return { name, type };
        }
      } else if (["/", "./", "~/"].every((prefix) => !path.startsWith(prefix))) {
        return { name, type };
      }
    }

    const type = SpecLocationSource.LOCAL;
    if (path.startsWith("/") || path.startsWith("~/")) {
      return { name: basename, type, path };
    }

    const relative = path.startsWith("./") ? path.slice(2) : path;
    return { name: basename, type, path: `${cwd}/${relative}` };
  };

  type ResolvedSpecLocation =
    | { type: "public"; name: string }
    | { type: "private"; namespace: string; name: string };

  const importSpecFromLocation = async (
    specLocation: SpecLocation,
    localLogger: Logger = logger,
  ): Promise<{
    specFile: SpecFileImport;
    resolvedLocation?: ResolvedSpecLocation;
  }> => {
    // Try loading spec from `devCompletionsFolder` first.
    const devPath = isInDevMode()
      ? (getSetting(SETTINGS.DEV_COMPLETIONS_FOLDER) as string)
      : undefined;

    const devPort = isInDevMode() ? getSetting(SETTINGS.DEV_COMPLETIONS_SERVER_PORT) : undefined;

    let specFile: SpecFileImport | undefined;
    let resolvedLocation: ResolvedSpecLocation | undefined;

    if (typeof devPort === "string" || typeof devPort === "number") {
      const { diffVersionedFile, name } = specLocation;
      specFile = await importFromLocalhost(
        diffVersionedFile ? `${name}/${diffVersionedFile}` : name,
        devPort,
      );
    }

    if (!specFile && devPath) {
      try {
        const { diffVersionedFile, name } = specLocation;
        const spec = await importSpecFromFile(
          diffVersionedFile ? `${name}/${diffVersionedFile}` : name,
          devPath,
          localLogger,
        );
        specFile = spec;
      } catch {
        // fallback to loading other specs in dev mode.
      }
    }

    if (!specFile && specLocation.type === SpecLocationSource.LOCAL) {
      // If we couldn't successfully load a dev spec try loading from specPath.
      const { name, path } = specLocation;
      const [dirname, basename] = splitPath(`${path || "~/"}${name}`);

      specFile = await importSpecFromFile(
        basename,
        `${dirname}.fig/autocomplete/build/`,
        localLogger,
      );
    } else if (!specFile) {
      const { name, diffVersionedFile: versionFileName } = specLocation;

      if (await bundledSpecExists(name)) {
        // If we're here, importing was successful.
        try {
          const result = await importBundledSpec(
            versionFileName ? `${name}/${versionFileName}` : name,
          );

          specFile = result;
          resolvedLocation = { type: "public", name };
        } catch (err) {
          localLogger.error("Unable to load from CDN", err);
          throw err;
        }
      } else {
        try {
          specFile = await importSpecFromFile(name, `~/.fig/autocomplete/build/`, localLogger);
        } catch (_err) {
          /* empty */
        }
      }
    }

    if (!specFile) {
      throw new MissingSpecError("No spec found");
    }

    return { specFile, resolvedLocation };
  };

  const loadFigSubcommand = async (
    specLocation: SpecLocation,
    _context?: Fig.ShellContext,
    localLogger: Logger = logger,
  ): Promise<Fig.Subcommand> => {
    const { name } = specLocation;
    const location = (await isDiffVersionedSpec(name))
      ? { ...specLocation, diffVersionedFile: "index" }
      : specLocation;
    const { specFile } = await importSpecFromLocation(location, localLogger);
    const subcommand = await tryResolveSpecToSubcommand(specFile, specLocation);
    return subcommand;
  };

  const loadSubcommandCached = async (
    specLocation: SpecLocation,
    context?: Fig.ShellContext,
    localLogger: Logger = logger,
  ): Promise<Subcommand> => {
    const { name, type: source } = specLocation;
    const path = specLocation.type === SpecLocationSource.LOCAL ? specLocation.path : "";

    // Do not load completion spec for commands that are 'disabled' by user
    const disabledSpecs = <string[]>getSetting(SETTINGS.DISABLE_FOR_COMMANDS) || [];
    if (disabledSpecs.includes(name)) {
      localLogger.info(`Not getting path for disabled spec ${name}`);
      throw new DisabledSpecError("Command requested disabled completion spec");
    }

    const key = [source, path || "", name].join(",");
    if (getSetting(SETTINGS.DEV_MODE_NPM_INVALIDATE_CACHE)) {
      specCache.clear();
      await setSetting(SETTINGS.DEV_MODE_NPM_INVALIDATE_CACHE, false);
    } else if (!getSetting(SETTINGS.DEV_MODE_NPM) && specCache.has(key)) {
      return specCache.get(key) as Subcommand;
    }

    const subcommand = await withTimeout(
      5000,
      loadFigSubcommand(specLocation, context, localLogger),
    );
    const converted = convertSubcommand(subcommand, initializeDefault);
    signal?.throwIfAborted();
    specCache.set(key, converted);
    return converted;
  };

  const tryResolveSpecToSubcommand = async (
    spec: SpecFileImport,
    location: SpecLocation,
  ): Promise<Fig.Subcommand> => {
    if (typeof spec.default === "function") {
      // Handle versioned specs, either simple versioned or diff versioned.
      const cliVersion = await getVersionFromFullFile(spec, location.name);
      const subcommandOrDiffVersionInfo = await Promise.resolve(spec.default(cliVersion));

      if ("versionedSpecPath" in subcommandOrDiffVersionInfo) {
        // Handle diff versioned specs.
        const { versionedSpecPath, version } = subcommandOrDiffVersionInfo;
        const [dirname, basename] = splitPath(versionedSpecPath);
        const { specFile } = await importSpecFromLocation({
          ...location,
          name: dirname.slice(0, -1),
          diffVersionedFile: basename,
        });

        if ("versions" in specFile) {
          const result = getVersionFromVersionedSpec(specFile.default, specFile.versions, version);
          return result.spec;
        }

        throw new WrongDiffVersionedSpecError("Invalid versioned specs file");
      }

      return subcommandOrDiffVersionInfo;
    }

    return spec.default;
  };

  return {
    getSpecPath,
    importSpecFromLocation,
    loadFigSubcommand,
    loadSubcommandCached,
    tryResolveSpecToSubcommand,
    clear: () => {
      specCache.clear();
      loader.clear();
    },
  };
}
