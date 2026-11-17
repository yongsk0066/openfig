import { importSpecSource } from "@fig/autocomplete-parser/browser";
import { History, Settings, Shell, fs } from "@fig/api-bindings";
import { executeCommandTimeout, executeLoginShell, fread } from "@fig/api-bindings/wrappers";
import { SETTINGS, readSettings, type SettingsMap } from "@fig/shared/settings";
import { withTimeout, runtimeTiming } from "@fig/shared/runtime-timing";
import type { HistoryPorts } from "../history/repository";
import { createCompletionCaches } from "../parser/cacheScopes";
import {
  createParser,
  createSpecLoader,
  createSpecRepository,
} from "@fig/autocomplete-parser/core";
import { createHistoryRepository } from "../history/repository";
import { createGeneratorCache, type GeneratorContext } from "../generators/helpers";
import { createRecencyRepository } from "../suggestions/sorting";
import { createGeneratorRunner } from "../state/generators";
import { createCompletionModel } from "../state/model";
import { applyEffect } from "./effects";
import { recordTiming } from "../telemetry";
import { runPipingConsoleMethods } from "../utils";
import logger from "loglevel";

type Context = Fig.ShellContext & { sessionId?: string; inputRevision?: bigint };
async function timedExecute(
  input: Fig.ExecuteCommandInput,
  timeout: number | undefined,
  signal: AbortSignal | undefined,
  context: Context,
) {
  const start = performance.now();
  try {
    return await executeCommandTimeout(input, timeout, signal, {
      sessionId: context.sessionId,
      cwd: context.currentWorkingDirectory || undefined,
      env: context.environmentVariables,
    });
  } finally {
    recordTiming(
      signal?.aborted ? "process-cancelled" : "process",
      performance.now() - start,
      context,
    );
  }
}
/** Composition root: only here do completion services acquire browser/desktop adapters. */
export function createDesktopCompletion() {
  let settings: SettingsMap = {};

  const getSetting = <T = unknown>(key: SETTINGS, fallback?: T): T =>
    readSettings(settings)(key, fallback);
  const cacheScopes = createCompletionCaches({ now: () => performance.now() });
  const scope = (context: Context, signal?: AbortSignal) => {
    const snapshot = settings;
    const read = readSettings(snapshot);
    const execution = {
      sessionId: context.sessionId,
      cwd: context.currentWorkingDirectory || undefined,
      env: context.environmentVariables,
    };
    const execute: Fig.ExecuteCommandFunction = (input) => {
      signal?.throwIfAborted();
      return timedExecute(input, 5000, signal, context);
    };
    const login: Parameters<typeof createParser>[0]["executeLoginShell"] = (input) =>
      executeLoginShell({ ...input, context: execution, signal });
    const isInDevMode = () => Boolean(read(SETTINGS.DEV_MODE) || read(SETTINGS.DEV_MODE_NPM));
    const caches = cacheScopes.scope(execution, snapshot);
    const loader = createSpecLoader(
      {
        withTimeout,
        executeCommand: execute,
        executeVersionCommand: (input) => timedExecute(input, 5000, undefined, context),
        fread,
        isInDevMode,
        importSource: importSpecSource,
        importModule: (url) => import(/* @vite-ignore */ url),
        readJson: async (url) => {
          const start = performance.now();
          try {
            return await fetch(url).then((response) => response.json());
          } finally {
            recordTiming("spec-fetch", performance.now() - start, context);
          }
        },
      },
      caches.loader,
    );
    const specs = createSpecRepository(
      {
        withTimeout,
        signal,
        loader,
        executeCommand: execute,
        getSetting: read,
        isInDevMode,
        setSetting: Settings.set,
      },
      caches.specs,
    );
    const parser = createParser(
      {
        withTimeout,
        signal,
        specs,
        executeCommand: execute,
        executeLoginShell: login,
        getSetting: read,
        isInDevMode,
      },
      caches.parser,
    );
    return { parser, specs, execute, login };
  };
  const emptyContext: Context = {
    currentWorkingDirectory: "",
    currentProcess: "zsh",
    environmentVariables: {},
    sshPrefix: "",
  };
  const historyPorts = (signal?: AbortSignal): HistoryPorts => ({
    timing: runtimeTiming,
    parseArguments: (...args) => scope(args[1], signal).parser.parseArguments(...args),
    getSpecPath: (...args) => scope(emptyContext, signal).specs.getSpecPath(...args),
    loadSubcommandCached: (...args) =>
      scope(args[1] ?? emptyContext, signal).specs.loadSubcommandCached(...args),
    executeCommand: (input) => scope(emptyContext, signal).execute(input),
    executeLoginShell: (input) => scope(emptyContext, signal).login(input),
    getSetting,
    query: (sql, params) => {
      signal?.throwIfAborted();
      return withTimeout(60_000, History.query(sql, params), signal);
    },
  });
  const history = createHistoryRepository(historyPorts(), historyPorts);
  const generatorCache = createGeneratorCache(getSetting, runtimeTiming.now);
  let storage: Storage | undefined;
  try {
    storage = window.localStorage;
  } catch {
    /* optional persistence */
  }
  const recency = createRecencyRepository(storage, Date.now, runtimeTiming.schedule);
  window.addEventListener("pagehide", recency.flush);
  const servicesFor = (context: GeneratorContext) => ({
    invoke: runPipingConsoleMethods,
    listDirectory: async (path: string, signal?: AbortSignal) => {
      signal?.throwIfAborted();
      const start = performance.now();
      try {
        return await withTimeout(5000, fs.list(path, true), signal);
      } finally {
        recordTiming("directory", performance.now() - start, context);
      }
    },
    execute: (input: Fig.ExecuteCommandInput, timeout?: number, signal?: AbortSignal) =>
      timedExecute(input, timeout, signal, context),
    cached: generatorCache.runCachedGenerator,
    history: history.getHistoryArgSuggestions,
    delay: runtimeTiming.delay,
    timeout: () => getSetting<number>(SETTINGS.SCRIPT_TIMEOUT, 5000),
  });
  const store = createCompletionModel({
    effect: applyEffect,
    generators: (write) => createGeneratorRunner(write, servicesFor),
    insertion: Shell.insert,
    recordUsage: recency.updateAutocompleteIndexFromUserInsert,
    history: history.getHistorySources,
    recency: recency.getRecencyIndex,
    now: () => performance.now(),
    measure: recordTiming,
    report: (error) => logger.warn(error),
  });
  return {
    store,
    parse: (
      command: Parameters<ReturnType<typeof createParser>["parseArguments"]>[0],
      context: Context,
      signal?: AbortSignal,
    ) => scope(context, signal).parser.parseArguments(command, context),
    settings(value: SettingsMap) {
      settings = { ...value };
      store.dispatch({ kind: "settings", update: settings });
    },
    async loadHistory(signal: AbortSignal) {
      signal.throwIfAborted();
      await history.loadHistory({}, signal, () => store.dispatch({ kind: "historyChanged" }));
      signal.throwIfAborted();
      store.dispatch({ kind: "historyChanged" });
    },
    async refreshHistory(signal: AbortSignal) {
      await history.refreshHistory({}, signal, () => store.dispatch({ kind: "historyChanged" }));
    },
    clear() {
      cacheScopes.clear();
      generatorCache.clear();
    },
    dispose() {
      window.removeEventListener("pagehide", recency.flush);
      recency.flush();
      store.dispose();
      cacheScopes.clear();
      generatorCache.clear();
    },
  };
}
