import { convertSubcommand, initializeDefault } from "@fig/spec-helpers";
import { SETTINGS, updateSettings } from "@fig/api-bindings/wrappers";
import { type ArgumentParserResult, TokenType } from "@fig/autocomplete-parser";
import { SuggestionFlag } from "@fig/shared/utils";
import { getCommand } from "@fig/shell-parser";
import logger from "loglevel";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useAutocompleteStore } from "..";
import { Visibility } from "../types";

const spec = convertSubcommand(
  { name: "tool", subcommands: [{ name: "build" }, { name: "bump" }, { name: "clean" }] },
  initializeDefault,
);

const parserResultFor = (searchTerm: string): ArgumentParserResult => ({
  completionObj: spec,
  currentArg: null,
  passedOptions: [],
  searchTerm,
  commandIndex: 0,
  suggestionFlags: SuggestionFlag.Any,
  annotations: [{ text: searchTerm, type: TokenType.None }],
});

const names = () => useAutocompleteStore.getState().suggestions.map((s) => s.name);

const initial = useAutocompleteStore.getState();

/** A fresh store that has shown the list for `tool <searchTerm>`. */
const typed = (searchTerm: string, parserResult = parserResultFor(searchTerm)) => {
  useAutocompleteStore.setState(initial, true);
  const buffer = `tool ${searchTerm}`;
  useAutocompleteStore.getState().setFigState((figState) => ({
    ...figState,
    buffer,
    cursorLocation: buffer.length,
    cwd: "/tmp",
  }));
  useAutocompleteStore.getState().setParserResult(parserResult, false, false);
};

describe("store: settings", () => {
  beforeEach(() => {
    updateSettings({});
    typed("build");
  });

  afterEach(() => updateSettings({}));

  it("a setting change recomputes suggestions on the same buffer", () => {
    const before = useAutocompleteStore.getState();
    expect(before.visibleState).toBe(Visibility.VISIBLE);
    expect(before.command).toEqual(getCommand("tool build", {}));
    // An exact match gets an auto-execute row in front of it.
    expect(before.suggestions.map((s) => s.type)).toEqual(["auto-execute", "subcommand"]);

    // Same order as `useFigSettings`: the module-level copy first, then the store.
    const settings = { [SETTINGS.HIDE_AUTO_EXECUTE_SUGGESTION]: true };
    updateSettings(settings);
    useAutocompleteStore.getState().setSettings(settings);

    expect(useAutocompleteStore.getState().suggestions.map((s) => s.type)).toEqual(["subcommand"]);
    expect(names()).toEqual([["build"]]);
  });

  it("the recompute reads the settings being written, not the previous ones", () => {
    expect(names()).toEqual([["build"], ["build"]]);

    // `beta.history.mode` is read from the store's settings. No history is loaded, so
    // `history_only` must empty the list in the same update.
    useAutocompleteStore.getState().setSettings({ [SETTINGS.HISTORY_MODE]: "history_only" });
    expect(names()).toEqual([]);

    useAutocompleteStore.getState().setSettings({ [SETTINGS.HISTORY_MODE]: "off" });
    expect(names()).toEqual([["build"], ["build"]]);
  });
});

describe("store: fuzzy search", () => {
  beforeEach(() => {
    updateSettings({});
    typed("bld");
  });

  it("toggling fuzzy search refilters the list on screen at once", () => {
    const { setUserFuzzySearchEnabled } = useAutocompleteStore.getState();
    expect(useAutocompleteStore.getState().visibleState).toBe(Visibility.VISIBLE);
    expect(names()).toEqual([]);

    // The `toggleFuzzySearch` keybinding; `autocomplete.fuzzySearch` changes go through the
    // same action (`App`).
    setUserFuzzySearchEnabled((enabled) => !enabled);
    expect(useAutocompleteStore.getState().fuzzySearchEnabled).toBe(true);
    expect(names()).toEqual([["build"]]);

    setUserFuzzySearchEnabled(false);
    expect(useAutocompleteStore.getState().fuzzySearchEnabled).toBe(false);
    expect(names()).toEqual([]);
  });

  it("a spec's filterStrategy still overrides the preference", () => {
    typed("bld", {
      ...parserResultFor("bld"),
      completionObj: { ...spec, filterStrategy: "prefix" },
    });
    useAutocompleteStore.getState().setUserFuzzySearchEnabled(true);
    expect(useAutocompleteStore.getState()).toMatchObject({
      userFuzzySearchEnabled: true,
      fuzzySearchEnabled: false,
    });
    expect(names()).toEqual([]);
  });
});

describe("store: logging", () => {
  it("a keystroke writes nothing to the console at the default log level", () => {
    logger.setLevel("warn");
    const spies = (["log", "group", "groupCollapsed", "groupEnd", "info", "debug"] as const).map(
      (method) => vi.spyOn(console, method),
    );
    const buffer = "tool b";
    useAutocompleteStore.getState().setFigState((figState) => ({
      ...figState,
      buffer,
      cursorLocation: buffer.length,
    }));
    useAutocompleteStore.getState().setParserResult(parserResultFor("b"), false, false);
    const calls = spies.map((spy) => [spy.getMockName(), spy.mock.calls.length]);
    spies.forEach((spy) => spy.mockRestore());
    expect(calls.filter(([, count]) => count !== 0)).toEqual([]);
  });
});
