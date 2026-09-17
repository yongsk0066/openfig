import { convertSubcommand, initializeDefault } from "@fig/spec-helpers";
import { SETTINGS, updateSettings } from "@fig/api-bindings/wrappers";
import { type ArgumentParserResult, TokenType } from "@fig/autocomplete-parser";
import { SuggestionFlag } from "@fig/shared/utils";
import { getCommand } from "@fig/shell-parser";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
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

describe("store: settings", () => {
  const initial = useAutocompleteStore.getState();

  beforeEach(() => {
    updateSettings({});
    useAutocompleteStore.setState(initial, true);
    const buffer = "tool build";
    useAutocompleteStore.getState().setFigState((figState) => ({
      ...figState,
      buffer,
      cursorLocation: buffer.length,
      cwd: "/tmp",
    }));
    useAutocompleteStore.getState().setParserResult(parserResultFor("build"), false, false);
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
});
