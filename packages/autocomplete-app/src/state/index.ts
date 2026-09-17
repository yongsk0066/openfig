import React from "react";
import logger from "loglevel";
import { create, type Mutate, type StateCreator, type StoreApi } from "zustand";

import { fieldsAreEqual } from "@fig/shared/utils";
import type { ArgumentParserResult } from "@fig/autocomplete-parser";
import type { SettingsMap } from "@fig/api-bindings/wrappers";
import { detailedDiff } from "deep-object-diff";
import type { FigState } from "../fig/hooks";
import { type AutocompleteState, type NamedSetState, Visibility } from "./types";

import { getHistorySources } from "../history";
import { getRecencyIndex } from "../suggestions/sorting";
import { applyEffect, type Effect, storeEffects } from "./effects";
import { createGeneratorRunner } from "./generators";
import { createInsertionState } from "./insertion";
import { computeSuggestions, type SuggestionsInput } from "./pipeline";
import {
  figStateTransition,
  initialState,
  parserResultTransition,
  type Transition,
} from "./transitions";

/*
 * The store holds inputs (edit buffer and shell context, settings, parser result, generator
 * results, visibility and history/fuzzy toggles) and the outputs computed from them
 * (`command`, `fuzzySearchEnabled`, `suggestions`, `selectedIndex`, `hasChangedIndex`).
 *
 * Data flow of one write:
 *   action → pure transition (./transitions)        → { state, effects }
 *          → updateSuggestions middleware: when the list may change,
 *            pure computeSuggestions (./pipeline)     → suggestions, selection
 *          → state written
 *          → effects run: intercept keystrokes (from the before/after states), shell
 *            globals and generator runs (from the transition).
 */

// zustand 5 types `replace` as a literal: `set(partial)` or `set(state, true)`.
const applySet = <T>(
  set: StoreApi<T>["setState"],
  value: T | Partial<T> | ((s: T) => T | Partial<T>),
  replace?: boolean,
) => {
  if (replace) {
    set(value as T | ((s: T) => T), true);
  } else {
    set(value as Partial<T> | ((s: T) => Partial<T>));
  }
};

/** The pipeline input for `updatedState`, with the list currently on screen in `state`. */
const suggestionsInput = (
  state: AutocompleteState,
  updatedState: AutocompleteState,
): SuggestionsInput => ({
  buffer: updatedState.figState.buffer,
  command: updatedState.command,
  context: { currentProcess: updatedState.figState.processUserIsIn || "" },
  parserResult: updatedState.parserResult,
  generatorStates: updatedState.generatorStates,
  settings: updatedState.settings,
  history: {
    modeEnabled: updatedState.historyModeEnabled,
    sources: getHistorySources(),
    recency: getRecencyIndex(),
  },
  fuzzy: updatedState.fuzzySearchEnabled,
  selection: {
    suggestions: state.suggestions,
    selectedIndex: state.selectedIndex,
    hasChangedIndex: state.hasChangedIndex,
    visible: updatedState.visibleState === Visibility.VISIBLE,
  },
});

const updateSuggestions =
  (creator: StateCreator<AutocompleteState>): StateCreator<AutocompleteState> =>
  (set, get, api) =>
    creator(
      (partial, replace) => {
        const state = get();
        const update = typeof partial === "function" ? partial(state) : partial;
        const updatedState = { ...state, ...update };

        if (logger.getLevel() <= logger.levels.DEBUG) {
          logger.debug({ diff: detailedDiff(state, updatedState) });
        }

        logger.debug({ state, updatedState });
        logger.info({ tokens: updatedState.command?.tokens });

        const suggestionsMayChange =
          !fieldsAreEqual(updatedState, state, [
            "command",
            "fuzzySearchEnabled",
            "parserResult",
            "generatorStates",
            "historyModeEnabled",
            "settings",
          ]) || !fieldsAreEqual(updatedState.figState, state.figState, ["cwd", "buffer"]);
        const wasVisible = state.visibleState === Visibility.VISIBLE;
        const willBeVisible = updatedState.visibleState === Visibility.VISIBLE;

        if (
          updatedState.visibleState === Visibility.HIDDEN_UNTIL_KEYPRESS ||
          (willBeVisible === wasVisible && !suggestionsMayChange)
        ) {
          applySet(set, updatedState, replace);
        } else {
          // Recompute suggestions if we change visibility state or will be visible and
          // suggestions might change.
          const output = computeSuggestions(suggestionsInput(state, updatedState));
          logger.info("Recomputed suggestions", { suggestions: output.suggestions });
          applySet(set, { ...updatedState, ...output }, replace);
        }

        storeEffects(state, get()).forEach(applyEffect);
      },
      get,
      api,
    );

type Get<T, K, F> = K extends keyof T ? T[K] : F;
type NamedStateCreator<T> = (
  setState: NamedSetState<T>,
  getState: Get<Mutate<StoreApi<T>, []>, "getState", never>,
  store: Mutate<StoreApi<T>, []>,
) => T;

const log =
  <T>(config: NamedStateCreator<T>): StateCreator<T, [], []> =>
  (set, get, api) => {
    const namedSet: NamedSetState<T> = (name, partial, replace) => {
      // Every store write goes through here (several per keystroke), so the console group
      // exists only when someone raised the log level to look at it.
      const grouped = logger.getLevel() <= logger.levels.DEBUG;
      if (grouped) console.groupCollapsed(`applying update: ${name}`);
      logger.info({ name, partial, replace });
      applySet(set, partial, replace);
      if (grouped) console.groupEnd();
    };
    return config(namedSet, get, api);
  };

export const useAutocompleteStore = create<AutocompleteState>()(
  updateSuggestions(
    log((setNamed, get) => {
      const generators = createGeneratorRunner(setNamed);
      const insertionState = createInsertionState(setNamed, get);

      const runEffects = (effects: readonly Effect[]) => {
        for (const effect of effects) {
          if (effect.kind === "runGenerator") {
            generators.runGenerator(effect);
          } else {
            applyEffect(effect);
          }
        }
      };

      /** Writes a transition's state, then runs its effects. */
      const dispatch = (name: string, transition: (state: AutocompleteState) => Transition) => {
        let effects: readonly Effect[] = [];
        setNamed(name, (state) => {
          const next = transition(state);
          effects = next.effects;
          return next.state;
        });
        runEffects(effects);
      };

      return {
        ...(initialState as AutocompleteState),
        ...insertionState,

        setParserResult: (
          parserResult: ArgumentParserResult,
          hasBackspacedToNewToken: boolean,
          largeBufferChange: boolean,
        ) =>
          dispatch("setParserResult", (state) =>
            parserResultTransition(state, parserResult, hasBackspacedToNewToken, largeBufferChange),
          ),

        scroll: (index: number, visibleState: Visibility) =>
          setNamed("scroll", (state) => {
            const selectedIndex = Math.max(Math.min(index, state.suggestions.length - 1), 0);
            return {
              selectedIndex,
              visibleState,
              hasChangedIndex: selectedIndex !== state.selectedIndex,
            };
          }),

        setVisibleState: (visibleState: Visibility) =>
          setNamed("setVisibleState", { visibleState }),

        setHistoryModeEnabled: (historyModeEnabled: React.SetStateAction<boolean>) =>
          setNamed("setHistoryModeEnabled", (state) => ({
            historyModeEnabled:
              typeof historyModeEnabled === "function"
                ? historyModeEnabled(state.historyModeEnabled)
                : historyModeEnabled,
          })),

        setUserFuzzySearchEnabled: (userFuzzySearchEnabled: React.SetStateAction<boolean>) =>
          setNamed("setUserFuzzySearchEnabled", (state) => ({
            userFuzzySearchEnabled:
              typeof userFuzzySearchEnabled === "function"
                ? userFuzzySearchEnabled(state.userFuzzySearchEnabled)
                : userFuzzySearchEnabled,
          })),

        setFigState: (newFigState: React.SetStateAction<FigState>) =>
          dispatch("setFigState", (state) => figStateTransition(state, newFigState)),

        error: (error: string) =>
          setNamed("error", (state) => {
            logger.warn(error);
            return {
              ...initialState,
              figState: state.figState,
              settings: state.settings,
            };
          }),

        setSettings: (settings: React.SetStateAction<SettingsMap>) =>
          setNamed("setSettings", (state) => ({
            settings: typeof settings === "function" ? settings(state.settings) : settings,
          })),
      };
    }),
  ),
);
