import type React from "react";
import logger from "loglevel";
import type { Suggestion } from "@fig/shared/internal";
import { fieldsAreEqual, memoizeOne } from "@fig/shared/utils";
import { type AliasMap, getCommand } from "@fig/shell-parser";
import { type ArgumentParserResult, initialParserState } from "@fig/autocomplete-parser";
import { getSetting, SETTINGS, type SettingsMap } from "@fig/api-bindings/wrappers";
import type { Types } from "@fig/api-bindings";
import { type FigState, initialFigState } from "../fig/hooks";
import type { GeneratorState } from "../generators/helpers";
import type { Effect, ShellGlobals } from "./effects";
import { planGenerators } from "./generators";
import { type AutocompleteState, Visibility } from "./types";

/**
 * A store update: the fields to write and the effects to run once they are written.
 * Transitions are pure — they read only their arguments (and the memoized, referentially
 * transparent helpers below) and never touch `window`, settings or IPC.
 */
export type Transition = {
  state: Partial<AutocompleteState>;
  effects: Effect[];
};

export const initialState: Partial<AutocompleteState> = {
  figState: initialFigState,
  parserResult: initialParserState,
  generatorStates: [] as GeneratorState[],
  command: null,

  visibleState: Visibility.HIDDEN_UNTIL_KEYPRESS,
  lastInsertedSuggestion: null,
  justInserted: false,

  selectedIndex: 0,
  suggestions: [] as Suggestion[],
  hasChangedIndex: false,

  historyModeEnabled: false,
  fuzzySearchEnabled: false,
  userFuzzySearchEnabled: getSetting(SETTINGS.FUZZY_SEARCH, false) as boolean,
  settings: {} as SettingsMap,
};

// Memoized so an unchanged buffer keeps the same `command` object: `useParseArgumentsEffect`
// re-parses whenever its identity changes.
const getCommandMemoized = memoizeOne(getCommand);

const getEnvVarsMemoized = memoizeOne(
  (arr: Types.EnvironmentVariable[]) =>
    Object.fromEntries(
      arr.map((pair) => [pair.key, pair.value]).filter((pair) => pair[0] && pair[1]),
    ),
  ([a], [b]) =>
    a.length === b.length &&
    a.every((elem, index) => elem.key === b[index].key && elem.value === b[index].value),
);

// `alias` output is `name=value` in bash and zsh, the only shells the integration installs
// into. The separator used to be chosen per shell because fish prints `name value`; the
// `shellPath` argument existed only for that test and went with the fish integration.
const getAliasMemoized = memoizeOne(
  (alias: string) =>
    alias
      .replace(/^alias\s/gm, "")
      .split("\n")
      .reduce((acc, a) => {
        try {
          const [key, ...value] = a.split("=");
          acc[key] = value.join("=").replace(/^'/, "").replace(/'$/, "");
        } catch (err) {
          logger.error(`Error parsing alias: ${a}`, err);
        }
        return acc;
      }, {} as AliasMap),
  ([aliasA], [aliasB]) => aliasA === aliasB,
);

/**
 * A new edit buffer / shell context. Tokenizes the buffer up to the cursor, and asks for the
 * shell globals that specs' `executeShellCommand` reads to be updated.
 *
 * IMPORTANT: any computation here must be memoized, otherwise it will trigger
 * setParserResult every time it's called.
 */
export const figStateTransition = (
  state: AutocompleteState,
  newFigState: React.SetStateAction<FigState>,
): Transition => {
  const nextFigState =
    typeof newFigState === "function" ? newFigState(state.figState) : newFigState;
  const figState = { ...nextFigState };

  const { sshContextString, cwd, buffer, cursorLocation, shellContext } = figState;
  const globals: ShellGlobals = {};

  // Set globalCWD and SSH string for completion specs that use executeShellCommand
  if (sshContextString) {
    globals.globalSSHString = sshContextString;
  }

  if (shellContext) {
    globals.globalTerminalSessionId = shellContext.sessionId;

    // protobuf-es decodes an absent repeated field as `[]`, which is truthy — so a
    // hook that deliberately omits the environment (figterm only re-sends it when the
    // shell context is dirty) would otherwise wipe the vars we already have.
    if (shellContext.environmentVariables?.length) {
      figState.environmentVariables = getEnvVarsMemoized(shellContext.environmentVariables);
    }

    if (shellContext.alias) {
      figState.aliases = getAliasMemoized(shellContext.alias);
    }
  }

  if (cwd) {
    globals.globalCWD = cwd;
  }

  const effects: Effect[] = [{ kind: "setShellGlobals", globals }];

  const error = (msg: string): Transition => {
    logger.debug(msg);
    return {
      state: {
        ...state,
        ...initialState,
        settings: state.settings,
        figState,
      },
      effects,
    };
  };

  const cursorChar = buffer.charAt(cursorLocation);
  if (cursorChar.trim()) {
    return error("Cursor is in the middle of a word.");
  }

  const bufferSliced = buffer.slice(0, cursorLocation);

  try {
    // Read after `shellContext.alias` is applied above: figterm sends the alias list only
    // when it changed, so the update that carries it must also expand its own buffer.
    const command = getCommandMemoized(bufferSliced, figState.aliases, cursorLocation);
    return { state: { figState, command }, effects };
  } catch (err) {
    return error(`Failed to get token array: ${err}`);
  }
};

/** Whether matching is fuzzy: the argument's spec decides, else the user's preference. */
const isFuzzySearchEnabled = (
  parserResult: ArgumentParserResult,
  userFuzzySearchEnabled: boolean,
): boolean => {
  switch (
    parserResult.currentArg
      ? parserResult.currentArg.filterStrategy
      : parserResult.completionObj.filterStrategy
  ) {
    case "prefix":
      return false;
    case "fuzzy":
      return true;
    case "default":
    default:
      return userFuzzySearchEnabled;
  }
};

/**
 * The user's fuzzy-search preference changed (the `toggleFuzzySearch` keybinding, or
 * `autocomplete.fuzzySearch` through `App`). Re-derives `fuzzySearchEnabled` for the current
 * argument right away, so the list on screen is refiltered by this write.
 */
export const userFuzzySearchTransition = (
  state: AutocompleteState,
  userFuzzySearchEnabled: React.SetStateAction<boolean>,
): Transition => {
  const enabled =
    typeof userFuzzySearchEnabled === "function"
      ? userFuzzySearchEnabled(state.userFuzzySearchEnabled)
      : userFuzzySearchEnabled;
  return {
    state: {
      userFuzzySearchEnabled: enabled,
      fuzzySearchEnabled: isFuzzySearchEnabled(state.parserResult, enabled),
    },
    effects: [],
  };
};

/**
 * A finished parse. Decides visibility and fuzzy matching for the new argument and which
 * generators to (re)run; the runs are returned as effects.
 */
export const parserResultTransition = (
  state: AutocompleteState,
  parserResult: ArgumentParserResult,
  hasBackspacedToNewToken: boolean,
  largeBufferChange: boolean,
): Transition => {
  let { visibleState, generatorStates } = state;
  const { settings } = state;

  // Trigger all generators on currentArg change.
  const hasNewArg = !fieldsAreEqual(parserResult, state.parserResult, [
    "currentArg",
    "completionObj",
  ]);

  logger.debug({ parserResult });

  const planned = planGenerators(state, parserResult);
  const newGeneratorStates = planned.generatorStates;
  if (
    newGeneratorStates.length !== generatorStates.length ||
    generatorStates.some((s, idx) => s !== newGeneratorStates[idx])
  ) {
    // Update if we've triggered at least one generator.
    generatorStates = newGeneratorStates;
  }

  const originalVisibleState = visibleState;
  switch (originalVisibleState) {
    case Visibility.HIDDEN_UNTIL_KEYPRESS: {
      visibleState = Visibility.VISIBLE;
      break;
    }
    case Visibility.HIDDEN_BY_INSERTION: {
      const insertionTriggeredGenerator = state.generatorStates.some(
        (oldState, idx) =>
          oldState.generator === state.lastInsertedSuggestion?.generator &&
          generatorStates[idx] !== oldState,
      );

      visibleState =
        hasNewArg || insertionTriggeredGenerator
          ? Visibility.VISIBLE
          : Visibility.HIDDEN_UNTIL_KEYPRESS;
      break;
    }
    default:
      break;
  }

  if ((settings[SETTINGS.ONLY_SHOW_ON_TAB] ?? false) as boolean) {
    visibleState = originalVisibleState;
    if (hasNewArg) {
      visibleState = Visibility.HIDDEN_UNTIL_SHOWN;
    }
  }

  if (hasBackspacedToNewToken) {
    visibleState = Visibility.HIDDEN_UNTIL_KEYPRESS;
  }

  if (largeBufferChange && !state.justInserted) {
    visibleState = Visibility.HIDDEN_UNTIL_KEYPRESS;
  }

  const userFuzzySearchEnabled = hasNewArg
    ? ((settings[SETTINGS.FUZZY_SEARCH] ?? false) as boolean)
    : state.userFuzzySearchEnabled;

  return {
    state: {
      parserResult,
      visibleState,
      fuzzySearchEnabled: isFuzzySearchEnabled(parserResult, userFuzzySearchEnabled),
      userFuzzySearchEnabled,
      generatorStates,
      justInserted: false,
    },
    effects: planned.effects,
  };
};
