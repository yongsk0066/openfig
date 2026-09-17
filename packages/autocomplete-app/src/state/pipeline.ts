import type { Suggestion } from "@fig/shared/internal";
import { fieldsAreEqual, makeArray } from "@fig/shared/utils";
import type { Command } from "@fig/shell-parser";
import type { ArgumentParserResult } from "@fig/autocomplete-parser";
import { SETTINGS, type SettingsMap } from "@fig/api-bindings/wrappers";
import type { GeneratorState } from "../generators/helpers";
import { getFullHistorySuggestions, type HistorySources } from "../history";
import { type RecencyIndex, updatePriorities } from "../suggestions/sorting";
import { deduplicateSuggestions, filterSuggestions, getAllSuggestions } from "../suggestions";

/**
 * Everything the suggestion list depends on. The store builds it from its own fields plus
 * the two long-lived indexes (`getHistorySources()`, `getRecencyIndex()`); tests build it
 * by hand.
 */
export type SuggestionsInput = {
  /** The whole edit buffer. */
  buffer: string;
  /** The buffer up to the cursor, tokenized with aliases expanded (`getCommand`). */
  command: Command | null;
  context: {
    /** Path of the process the user is in; its basename picks the shell history. */
    currentProcess: string;
  };
  parserResult: ArgumentParserResult;
  generatorStates: readonly GeneratorState[];
  settings: SettingsMap;
  history: {
    /** Toggled by the history-mode keybinding (independent of `beta.history.mode`). */
    modeEnabled: boolean;
    sources: HistorySources;
    recency: RecencyIndex;
  };
  /** Whether matching is fuzzy for this argument (`fuzzySearchEnabled`). */
  fuzzy: boolean;
  /** The list on screen now: a selection the user moved follows its item into the new list. */
  selection: {
    suggestions: readonly Suggestion[];
    selectedIndex: number;
    hasChangedIndex: boolean;
    /** Whether the list will be visible after this update. */
    visible: boolean;
  };
};

export type SuggestionsOutput = {
  suggestions: Suggestion[];
  selectedIndex: number;
  hasChangedIndex: boolean;
};

const rankSuggestions = (input: SuggestionsInput): Suggestion[] => {
  const { parserResult, command, settings, history } = input;
  const { annotations } = parserResult;
  const rootCommand = command?.tokens[0]?.text ?? "";
  const sortMethod = settings[SETTINGS.SORT_METHOD];

  // A scan over every history entry; only the three history branches below use it.
  const getHistorySuggestions = () =>
    getFullHistorySuggestions(
      input.buffer,
      command,
      input.context.currentProcess,
      history.sources,
      settings,
    );

  const specSuggestions = getAllSuggestions(
    parserResult.currentArg,
    parserResult.completionObj,
    parserResult.passedOptions,
    parserResult.suggestionFlags,
    input.generatorStates,
    annotations,
  );

  let suggestions = specSuggestions;
  const historyMode = settings[SETTINGS.HISTORY_MODE] as string;
  if (historyMode === "show") {
    const historySuggestions = getHistorySuggestions();
    const existingNames = new Set();
    suggestions.forEach((suggestion) => {
      makeArray(suggestion.name || []).forEach((name) => existingNames.add(name || ""));
    });
    // Remove suggestions whose names are already in fig's suggestions
    suggestions.push(...historySuggestions.filter((x) => !existingNames.has(x.name)));

    const historyPriorities: Record<string, number> = {};
    for (let i = 0; i < historySuggestions.length; i += 1) {
      const name = makeArray(historySuggestions[i].name)[0] || "";
      historyPriorities[name.split(" ", 1)[0]] = historySuggestions[i].priority || 50;
    }
    suggestions = updatePriorities(suggestions, rootCommand, sortMethod, history.recency);

    // Take highest priority if suggestion was in historySuggestions
    suggestions = suggestions.map((suggestion) => ({
      ...suggestion,
      priority: Math.max(
        suggestion.priority || 0,
        ...makeArray(suggestion.name).map((name) => historyPriorities[name || ""] || 0),
      ),
    }));
  } else if (historyMode === "history_only") {
    suggestions = getHistorySuggestions();
  } else if (history.modeEnabled) {
    suggestions = getHistorySuggestions();
  }

  suggestions = updatePriorities(suggestions, rootCommand, sortMethod, history.recency).sort(
    (a, b) => (b.priority || 0) - (a.priority || 0),
  );

  const filtered = filterSuggestions(
    suggestions,
    parserResult.searchTerm,
    input.fuzzy,
    parserResult.currentArg?.suggestCurrentToken ??
      ((settings[SETTINGS.ALWAYS_SUGGEST_CURRENT_TOKEN] ?? false) as boolean),
    settings,
  );

  // Deduplication is relatively slow, so we only do it if there aren't
  // enough suggestions to cause any noticeable performance issues.
  if (filtered.length > 0 && filtered.length <= 50) {
    return deduplicateSuggestions(filtered, ["name", "insertValue", "displayName", "args"]);
  }
  return filtered;
};

/**
 * The suggestion list for one state of the buffer, parser and generators. Pure: no store,
 * no module-level settings, no I/O — the same input always gives an equal output. (The
 * static spec suggestions are memoized by argument identity, which does not change that.)
 */
export const computeSuggestions = (input: SuggestionsInput): SuggestionsOutput => {
  const suggestions = rankSuggestions(input);

  // If the list was visible and the user moved the selection, keep the same item selected.
  const { selection } = input;
  let selectedIndex = 0;
  let hasChangedIndex = false;
  const lastSelected = selection.suggestions[selection.selectedIndex];

  if (selection.hasChangedIndex && selection.visible && lastSelected) {
    const newIndex = suggestions.findIndex((suggestion: Suggestion) =>
      fieldsAreEqual(suggestion, lastSelected, ["name", "type", "insertValue", "description"]),
    );
    if (newIndex !== -1) {
      selectedIndex = newIndex;
      hasChangedIndex = selection.hasChangedIndex;
    }
  }

  return { suggestions, selectedIndex, hasChangedIndex };
};
