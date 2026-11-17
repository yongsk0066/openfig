import type { Timing } from "@fig/shared/timing";
import { LruCache } from "@fig/shared/lru";
import { createPrefixIndex } from "./prefix-index";
import type { Internal } from "@fig/spec-helpers";
import { getLogger } from "loglevel";
import {
  TokenType,
  flattenAnnotations,
  type BasicAnnotation,
  type Annotation,
  serializeSpecLocation,
} from "@fig/autocomplete-parser/core";
import { getAllCommandsWithAlias, type AliasMap, type Command } from "@fig/shell-parser";
import { SpecLocationSource } from "@fig/shared/utils";
import type { Suggestion } from "@fig/shared/internal";
import { SETTINGS, type SettingsMap, type SettingsReader } from "@fig/shared/settings";
import type {
  ParserPorts,
  createParser,
  createSpecRepository,
} from "@fig/autocomplete-parser/core";
import {
  type AnnotatedCommand,
  type HistoryEntry,
  type Graph,
  mirrorSubcommand,
  walkSubcommand,
  type HistoryContext,
} from "./helpers";
// Keep a transition token in its parent segment before starting the replacement segment.
// This preserves command/option argument history without shifting parser token indices.
function historySegments(annotations: BasicAnnotation[]) {
  const segments: {
    spec: Extract<BasicAnnotation, { spec: unknown }>["spec"];
    key: string;
    annotations: BasicAnnotation[];
  }[] = [];
  for (const annotation of annotations) {
    if ("spec" in annotation) {
      segments.push({
        spec: annotation.spec,
        key: serializeSpecLocation(annotation.specLocation),
        annotations: [],
      });
    } else {
      segments.at(-1)?.annotations.push(annotation);
    }
    if (annotation.loadedSpec) {
      segments.push({
        spec: annotation.loadedSpec.spec,
        key: annotation.loadedSpec.historyKey,
        annotations: [],
      });
    }
  }
  return segments;
}
const awaitBatched = async <T>(
  batchSize: number,
  getPromiseArray: Array<() => Promise<T>>,
  signal?: AbortSignal,
): Promise<T[]> => {
  const results: T[] = [];
  for (let i = 0; i < getPromiseArray.length; i += batchSize) {
    signal?.throwIfAborted();
    const promises = getPromiseArray.slice(i, i + batchSize).map((getPromise) => getPromise());
    const batchResults = await Promise.all(promises);
    signal?.throwIfAborted();
    results.push(...batchResults);
  }
  return results;
};
const indexArgumentValues = (
  historyValueIndex: HistoryValueIndex,
  entry: HistoryEntry,
  entryIndex: number,
) => {
  const { commands } = entry;
  commands.forEach(({ parserResult }) => {
    if (parserResult) {
      const annotations = flattenAnnotations(parserResult.annotations);

      for (const { spec, key: specKey, annotations: other } of historySegments(annotations)) {
        if (!historyValueIndex[specKey]) {
          historyValueIndex[specKey] = mirrorSubcommand(
            spec,
            (arg) => ({ values: [], isVariadic: arg.isVariadic }) as HistoryArgValues,
            () => ({}),
            () => ({}),
          );
        }

        try {
          walkSubcommand(historyValueIndex[specKey], other, {
            arg: (arg, annotation) => {
              arg.values.push({
                historyEntryIndex: entryIndex,
                value: annotation.text,
              });
            },
          });
        } catch (err) {
          historyLogger.error("Error walking subcommand:", err);
        }
      }
    }
  });
};

const historyLogger = getLogger("history");
historyLogger.setDefaultLevel("warn");

type HistoryArgValues = {
  isVariadic?: boolean;
  values: { historyEntryIndex: number; value: string }[];
};

type HistoryValueIndex = Record<string, Internal.Subcommand<HistoryArgValues, unknown, unknown>>;

export type HistorySource = {
  entries: HistoryEntry[];
  entryGraph?: Graph;
  subcommandGraph?: Graph;
  historyValueIndex?: HistoryValueIndex;
};

export type HistorySources = Readonly<Record<string, HistorySource>>;

const historyPrefix = createPrefixIndex<HistoryEntry>();

const getFirstWord = (text: string) => text.split(" ", 1)[0] || "";

export const getHistoryArgSuggestions = (
  annotations: Annotation[],
  currentProcess: string,
  historySources: HistorySources,
  settings: SettingsMap,
): Fig.TemplateSuggestion[] => {
  const shell = currentProcess.slice(currentProcess.lastIndexOf("/") + 1);
  const basicAnnotations = flattenAnnotations(annotations);
  const segment = historySegments(basicAnnotations).at(-1);
  if (!segment || segment.annotations.length === 0) return [];
  const { key: specKey, annotations: specAnnotations } = segment;

  const getValuesFromSource = (source: HistorySource) => {
    const subcommand = source.historyValueIndex?.[specKey];
    if (!subcommand) {
      return [];
    }

    const getValuesForArgType = (
      type: TokenType.OptionArg | TokenType.SubcommandArg,
    ): Fig.TemplateSuggestion[] => {
      const result = walkSubcommand(subcommand, [
        ...specAnnotations.slice(0, -1),
        { text: "", type },
      ]);
      historyLogger.info({ result, type });
      if (result.type === type) {
        return (
          result.arg?.values
            ?.filter((item) => item.value)
            .map((item) => {
              const { historyEntryIndex, value } = item;
              const { context } = source.entries[historyEntryIndex];
              return {
                name: value,
                type: "arg",
                context: {
                  templateType: "history",
                  ...context,
                },
              };
            }) ?? []
        );
      }
      return [];
    };

    const type = specAnnotations.at(-1)?.type;
    if (type === TokenType.SubcommandArg || type === TokenType.OptionArg)
      return getValuesForArgType(type);
    return [
      ...getValuesForArgType(TokenType.SubcommandArg),
      ...getValuesForArgType(TokenType.OptionArg),
    ];
  };

  const allValues: Fig.TemplateSuggestion[] = [];

  if (settings[SETTINGS.HISTORY_COMMAND]) {
    allValues.push(...getValuesFromSource(historySources.customCommand));
  } else if (settings[SETTINGS.HISTORY_MERGE_SHELLS] && ["bash", "zsh"].includes(shell)) {
    allValues.push(
      ...getValuesFromSource(historySources.zsh),
      ...getValuesFromSource(historySources.bash),
    );
  } else if (shell in historySources) {
    allValues.push(...getValuesFromSource(historySources[shell]));
  }

  if (allValues.length === 0) {
    allValues.push(...getValuesFromSource(historySources.fig));
  }

  return allValues;
};

/** Pure: reads only its arguments (`sources` is `getHistorySources()` in the app). */
export const getFullHistorySuggestions = (
  buffer: string,
  command: Command | null,
  currentProcess: string,
  sources: HistorySources,
  settings: SettingsMap,
): Suggestion[] => {
  const shell = currentProcess.slice(currentProcess.lastIndexOf("/") + 1);
  const tokens = command?.tokens ?? [];
  const searchTermToken = tokens[tokens.length - 1];
  const prefixLength = searchTermToken?.originalNode
    ? searchTermToken.originalNode.startIndex +
      searchTermToken.originalNode.text.length -
      searchTermToken.originalNode.innerText.length
    : 0;
  const prefix = buffer.slice(0, prefixLength);
  const frequencies: Record<string, number> = {};
  const suggestions: string[] = [];
  const historyEntries: HistoryEntry[] = [];

  if (settings[SETTINGS.HISTORY_COMMAND]) {
    historyEntries.push(...historyPrefix(sources.customCommand.entries, prefix));
  } else if (settings[SETTINGS.HISTORY_MERGE_SHELLS] && ["bash", "zsh"].includes(shell)) {
    historyEntries.push(
      ...historyPrefix(sources.zsh.entries, prefix),
      ...historyPrefix(sources.bash.entries, prefix),
    );
  } else if (shell in sources) {
    historyEntries.push(...historyPrefix(sources[shell].entries, prefix));
  }

  for (let i = 0; i < historyEntries.length; i += 1) {
    const { text } = historyEntries[i];
    if (text.length >= prefix.length && text.startsWith(prefix)) {
      const name = text.slice(prefix.length).trimEnd();
      suggestions.push(name);

      const firstWord = getFirstWord(name);
      if (firstWord) {
        if (!frequencies[firstWord]) {
          frequencies[firstWord] = 0;
        }
        frequencies[firstWord] += 1;
      }
    }
  }

  return [...new Set(suggestions)].map((suggestion) => {
    const frequency = frequencies[getFirstWord(suggestion)] || 0;
    return {
      type: "history",
      description: "past command",
      name: suggestion,
      insertValue: suggestion,
      icon: "📚",
      priority: frequency > 1 ? 75 + Math.min(frequency, 10) / 10 : 50,
    };
  });
};

export type HistoryPorts = {
  timing: Pick<Timing, "now" | "delay">;
  parseArguments: ReturnType<typeof createParser>["parseArguments"];
  getSpecPath: ReturnType<typeof createSpecRepository>["getSpecPath"];
  loadSubcommandCached: ReturnType<typeof createSpecRepository>["loadSubcommandCached"];
  executeCommand: Fig.ExecuteCommandFunction;
  executeLoginShell: ParserPorts["executeLoginShell"];
  getSetting: SettingsReader;
  query(sql: string, params: string[]): Promise<Record<string, unknown>[]>;
};
const mapRows = (rows: Record<string, unknown>[]): HistoryEntry[] =>
  rows.map((entry) => ({
    text: entry.command as string,
    context: {
      cwd: entry.cwd,
      shell: entry.shell,
      exitCode: entry.exit_code,
      terminalSession: entry.session_id,
      time: entry.start_time,
    } as HistoryContext,
    commands: [],
  }));

export function createHistoryRepository(
  ports: HistoryPorts,
  scoped: (signal?: AbortSignal) => HistoryPorts = () => ports,
) {
  const { getSetting } = ports;
  const parsedEntries = new LruCache<string, HistoryEntry>(4096);
  let historySources: Record<string, HistorySource> = {
    zsh: { entries: [] },
    fig: { entries: [] },
    bash: { entries: [] },
    customCommand: { entries: [] },
  };

  /** The live history sources; `loadHistory` replaces their entries once loaded. */
  const getHistorySources = (): HistorySources => historySources;
  const loadHistorySource = async (
    items: HistoryEntry[],
    aliases: AliasMap,
    signal?: AbortSignal,
  ): Promise<HistorySource> => {
    signal?.throwIfAborted();
    const { getSpecPath, loadSubcommandCached, parseArguments } = scoped(signal);
    const entryMap: Record<string, HistoryEntry> = {};
    const aliasKey = JSON.stringify(aliases);
    const allLocations: string[] = [];

    const fakeCWD = "_fig/fake_cwd";
    const getCommandLocation = async (
      command: AnnotatedCommand,
      context?: Partial<HistoryContext>,
    ) => getSpecPath(command.command.tokens[0]?.text ?? "", context?.cwd ?? fakeCWD);

    for (let i = 0; i < items.length; i += 1) {
      signal?.throwIfAborted();
      const { text, context } = items[i];
      const entryKey = JSON.stringify([text, context?.cwd, aliasKey]);
      if (!(entryKey in entryMap)) {
        const cached = parsedEntries.get(entryKey);
        if (cached) {
          entryMap[entryKey] = cached;
          continue;
        }
        let commands: AnnotatedCommand[] = [];
        try {
          commands = getAllCommandsWithAlias(text, aliases).map((command) => ({
            command,
          }));
        } catch (err) {
          historyLogger.warn("Error getting commands in history", err);
        }
        entryMap[entryKey] = { text, commands };

        for (const command of commands) {
          const location = await getCommandLocation(command, context);
          if (location.type === SpecLocationSource.GLOBAL) {
            allLocations.push(location.name);
          }
        }
      }
    }

    const locations = [...new Set(allLocations)];
    const locationPromises = locations.map(
      (name) => () =>
        loadSubcommandCached(
          { name, type: SpecLocationSource.GLOBAL },
          undefined,
          historyLogger,
        ).catch((e) => e),
    );

    // Pre-load all specs from history.
    await awaitBatched(10, locationPromises, signal);

    const entries: HistoryEntry[] = [];
    for (let i = 0; i < items.length; i += 1) {
      signal?.throwIfAborted();
      const { text, context } = items[i];
      const entryKey = JSON.stringify([text, context?.cwd, aliasKey]);
      const entry = { ...entryMap[entryKey], context };
      for (let j = 0; j < entry.commands.length; j += 1) {
        const command = entry.commands[j];
        if (command.parserResult) continue;
        const location = await getCommandLocation(command, context);
        if (location.type === SpecLocationSource.GLOBAL) {
          try {
            const parseContext: Fig.ShellContext = {
              currentWorkingDirectory: context?.cwd ?? fakeCWD,
              sshPrefix: "",
              currentProcess: "",
              environmentVariables: {},
            };
            command.parserResult = await parseArguments(
              command.command,
              parseContext,
              true,
              historyLogger,
            );
          } catch (_err) {
            signal?.throwIfAborted();
            // skip errors in parsing commands.
          }
        }
      }
      signal?.throwIfAborted();
      entries.push(entry);
      parsedEntries.set(entryKey, entryMap[entryKey]);
    }

    const historyValueIndex = {};
    entries.forEach((entry, index) => {
      try {
        indexArgumentValues(historyValueIndex, entry, index);
      } catch (err) {
        historyLogger.warn({ err });
      }
    });

    return { entries, historyValueIndex };
  };

  let lastId = 0;
  let initialized = false;
  let queue = Promise.resolve();
  const serialize = (run: () => Promise<void>) => {
    const result = queue.then(run);
    queue = result.catch(() => {});
    return result;
  };
  const publish = (entries: HistoryEntry[], changed: () => void) => {
    const historyValueIndex = {};
    entries.forEach((entry, index) => indexArgumentValues(historyValueIndex, entry, index));
    historySources = { ...historySources, fig: { entries, historyValueIndex } };
    changed();
  };
  const readNewRows = async (aliases: AliasMap, signal?: AbortSignal, changed = () => {}) => {
    if (!initialized) return;
    const { query } = scoped(signal);
    for (;;) {
      signal?.throwIfAborted();
      const rows = await query(
        "SELECT id, command, shell, session_id, cwd, start_time, exit_code FROM history WHERE id > ? ORDER BY id ASC LIMIT 500",
        [String(lastId)],
      );
      signal?.throwIfAborted();
      if (!rows.length) break;
      const source = await loadHistorySource(mapRows(rows), aliases, signal);
      signal?.throwIfAborted();
      for (const shell of ["zsh", "bash"]) {
        const additions = source.entries.filter(
          (entry) => entry.context?.shell?.split("/").at(-1) === shell,
        );
        if (!additions.length) continue;
        const entries = [...historySources[shell].entries, ...additions];
        const historyValueIndex = {};
        entries.forEach((entry, index) => indexArgumentValues(historyValueIndex, entry, index));
        historySources = { ...historySources, [shell]: { entries, historyValueIndex } };
      }
      publish([...historySources.fig.entries, ...source.entries], changed);
      lastId = Number(rows[rows.length - 1].id);
      if (rows.length < 500) break;
    }
  };
  const refreshHistory = (aliases: AliasMap, signal?: AbortSignal, changed?: () => void) =>
    serialize(() => readNewRows(aliases, signal, changed));
  const loadHistory = (aliases: AliasMap, signal?: AbortSignal, changed = () => {}) =>
    serialize(async () => {
      signal?.throwIfAborted();
      parsedEntries.clear();
      const { query, executeLoginShell, executeCommand } = scoped(signal);
      const loadHistoryCommand = async (command: string, shell?: string): Promise<void> => {
        signal?.throwIfAborted();
        let items: HistoryEntry[] = [];
        try {
          const history = shell
            ? await executeLoginShell({ command, shell })
            : (await executeCommand({ command: "zsh", args: ["-c", command] })).stdout;
          signal?.throwIfAborted();
          items = history.split("\n").map((text) => ({ text, context: {}, commands: [] }));
        } catch (err) {
          signal?.throwIfAborted();
          historyLogger.error(err as Error);
        }
        const result = await loadHistorySource(items, aliases, signal);
        signal?.throwIfAborted();
        historySources = { ...historySources, [shell ?? "customCommand"]: result };
        changed();
      };
      // Publish recent rows first. Keyset pagination bounds each query and freezes the snapshot
      // against concurrent inserts; the final catch-up incorporates commits during startup.
      const [watermark] = await query("SELECT COALESCE(MAX(id), 0) AS id FROM history", []);
      signal?.throwIfAborted();
      const high = Number(watermark.id);
      let before = high + 1;
      let lastYield = ports.timing.now();
      let entries: HistoryEntry[] = [];
      while (before > 1) {
        signal?.throwIfAborted();
        const rows = await query(
          "SELECT id, command, shell, session_id, cwd, start_time, exit_code FROM history WHERE id < ? ORDER BY id DESC LIMIT 500",
          [String(before)],
        );
        signal?.throwIfAborted();
        if (!rows.length) break;
        const source = await loadHistorySource(mapRows(rows.toReversed()), aliases, signal);
        signal?.throwIfAborted();
        entries = [...source.entries, ...entries];
        publish(entries, changed);
        before = Number(rows[rows.length - 1].id);
        if (rows.length < 500) break;
        // IPC normally yields already; bound CPU slices without a timer for every small batch.
        if (ports.timing.now() - lastYield >= 8) {
          await ports.timing.delay(0, signal);
          lastYield = ports.timing.now();
        }
      }
      signal?.throwIfAborted();
      if (!entries.length) publish([], changed);
      lastId = high;
      initialized = true;
      const command = getSetting<string>(SETTINGS.HISTORY_COMMAND);
      if (command) {
        await loadHistoryCommand(command);
      } else {
        await loadHistoryCommand("fc -R; fc -ln 1", "zsh").catch(() => {});
        await loadHistoryCommand("fc -ln 1", "bash").catch(() => {});
      }
      signal?.throwIfAborted();
      await readNewRows(aliases, signal, changed);
    });

  return {
    refreshHistory,
    getHistorySources,
    loadHistorySource,
    loadHistory,
    getHistoryArgSuggestions: (annotations: Annotation[], currentProcess: string) =>
      getHistoryArgSuggestions(annotations, currentProcess, historySources, {
        [SETTINGS.HISTORY_COMMAND]: getSetting(SETTINGS.HISTORY_COMMAND),
        [SETTINGS.HISTORY_MERGE_SHELLS]: getSetting(SETTINGS.HISTORY_MERGE_SHELLS),
      }),
  };
}
