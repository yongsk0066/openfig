import { createStore, type StoreApi } from "zustand/vanilla";
import { fieldsAreEqual } from "@fig/shared/utils";
import type { ArgumentParserResult } from "@fig/autocomplete-parser/core";
import { SETTINGS, type SettingsMap } from "@fig/shared/settings";
import type { FigState } from "./fig-state";
import { Visibility, type AutocompleteState, type NamedSetState, type Update } from "./types";
import {
  interceptKeystrokesEffect,
  storeEffects,
  type Effect,
  type RunGeneratorEffect,
} from "./effects";
import { createInsertionState } from "./insertion";
import { createSuggestionComputer, type SuggestionsInput } from "./pipeline";
import {
  figStateTransition,
  initialState,
  parserResultTransition,
  userFuzzySearchTransition,
  type Transition,
} from "./transitions";
const resolveStateUpdate = <T>(update: Update<T>, current: T): T =>
  typeof update === "function" ? (update as (value: T) => T)(current) : update;

export type CompletionEvent =
  | { kind: "input"; update: Update<FigState> }
  | { kind: "settings"; update: Update<SettingsMap> }
  | { kind: "parsing"; value: boolean }
  | {
      kind: "parsed";
      result: ArgumentParserResult;
      backspaced: boolean;
      large: boolean;
      input?: AutocompleteState["parsedInput"];
    }
  | { kind: "failed"; error: string }
  | { kind: "historyChanged" };
export type ModelPorts = {
  effect(effect: Exclude<Effect, RunGeneratorEffect>): void;
  generators(write: NamedSetState<AutocompleteState>): {
    reconcile(states: AutocompleteState["generatorStates"]): void;
    runGenerator(effect: RunGeneratorEffect): void;
  };
  insertion: Parameters<typeof createInsertionState>[2];
  recordUsage(command: string, suggestion: string): void;
  history(): SuggestionsInput["history"]["sources"];
  recency(): SuggestionsInput["history"]["recency"];
  now(): number;
  measure(stage: string, milliseconds: number, context?: AutocompleteState["parsedInput"]): void;
  report(error: unknown): void;
};

/** One commit path owns derived suggestions, generator reconciliation and effects. */
export function createCompletionModel(ports: ModelPorts) {
  const compute = createSuggestionComputer();
  let disposed = false;
  const commit: NamedSetState<AutocompleteState> = (_name, partial, replace) => {
    if (disposed) return;
    const before = store.getState();
    const update = typeof partial === "function" ? partial(before) : partial;
    let after = { ...before, ...update };
    const changed =
      _name === "historyChanged" ||
      !fieldsAreEqual(after, before, [
        "command",
        "fuzzySearchEnabled",
        "parserResult",
        "generatorStates",
        "historyModeEnabled",
        "settings",
      ]) ||
      !fieldsAreEqual(after.figState, before.figState, ["cwd", "buffer"]);
    if (
      after.visibleState !== Visibility.HIDDEN_UNTIL_KEYPRESS &&
      (changed || after.visibleState !== before.visibleState)
    ) {
      const start = ports.now();
      const result = compute({
        buffer: after.figState.buffer,
        command: after.command,
        context: { currentProcess: after.figState.processUserIsIn || "" },
        parserResult: after.parserResult,
        generatorStates: after.generatorStates,
        settings: after.settings,
        history: {
          modeEnabled: after.historyModeEnabled,
          sources: ports.history(),
          recency: ports.recency(),
        },
        fuzzy: after.fuzzySearchEnabled,
        selection: {
          suggestions: before.suggestions,
          selectedIndex: before.selectedIndex,
          hasChangedIndex: before.hasChangedIndex,
          visible: after.visibleState === Visibility.VISIBLE,
        },
      });
      after = { ...after, ...result };
      ports.measure("ranking", ports.now() - start, after.figState.shellContext);
    }
    if (replace) store.setState(after, true);
    else store.setState(after);
    generators.reconcile(store.getState().generatorStates);
    storeEffects(before, store.getState()).forEach(ports.effect);
  };
  const generators = ports.generators(commit);
  const transition = (name: string, reduce: (state: AutocompleteState) => Transition) => {
    const result = reduce(store.getState());
    commit(name, result.state);
    for (const effect of result.effects) {
      if (disposed) break;
      if (effect.kind === "runGenerator") generators.runGenerator(effect);
      else ports.effect(effect);
    }
  };
  const dispatch = (event: CompletionEvent) => {
    if (disposed) return;
    switch (event.kind) {
      case "input":
        transition(event.kind, (s) => figStateTransition(s, event.update));
        break;
      case "settings": {
        const settings = resolveStateUpdate(event.update, store.getState().settings);
        transition(event.kind, (s) => {
          const result = userFuzzySearchTransition(s, Boolean(settings[SETTINGS.FUZZY_SEARCH]));
          return { ...result, state: { ...result.state, settings } };
        });
        break;
      }
      case "parsing":
        commit(event.kind, { parsing: event.value });
        break;
      case "parsed":
        transition(event.kind, (s) => {
          const result = parserResultTransition(s, event.result, event.backspaced, event.large);
          return {
            ...result,
            state: { ...result.state, parsing: false, parsedInput: event.input },
          };
        });
        break;
      case "failed":
        ports.report(event.error);
        commit(event.kind, (s) => ({
          ...initialState,
          figState: s.figState,
          settings: s.settings,
        }));
        break;
      case "historyChanged":
        commit(event.kind, {});
        break;
    }
  };
  const store: StoreApi<AutocompleteState> = createStore<AutocompleteState>(() => ({
    ...(initialState as AutocompleteState),
    ...createInsertionState(
      commit,
      () => store.getState(),
      ports.insertion,
      (command, suggestion) => {
        if (!disposed) ports.recordUsage(command, suggestion);
      },
    ),
    setFigState: (update) => dispatch({ kind: "input", update }),
    setSettings: (update) => dispatch({ kind: "settings", update }),
    setParserResult: (result, backspaced, large) =>
      dispatch({ kind: "parsed", result, backspaced, large }),
    error: (error) => dispatch({ kind: "failed", error }),
    setVisibleState: (visibleState) => commit("visibility", { visibleState }),
    scroll: (index, visibleState) =>
      commit("selection", (s) => {
        const selectedIndex = Math.max(Math.min(index, s.suggestions.length - 1), 0);
        return { selectedIndex, visibleState, hasChangedIndex: selectedIndex !== s.selectedIndex };
      }),
    setHistoryModeEnabled: (update) =>
      commit("historyMode", (s) => ({
        historyModeEnabled: resolveStateUpdate(update, s.historyModeEnabled),
      })),
    setUserFuzzySearchEnabled: (update) =>
      transition("fuzzy", (s) => userFuzzySearchTransition(s, update)),
  }));
  return Object.assign(store, {
    dispatch,
    syncEffects: () => {
      if (!disposed) ports.effect(interceptKeystrokesEffect(store.getState()));
    },
    cancelGenerators: () => commit("cancelGenerators", { generatorStates: [], parsing: false }),
    dispose: () => {
      disposed = true;
      generators.reconcile([]);
    },
  });
}
