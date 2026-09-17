import { convertSubcommand, initializeDefault } from "@fig/spec-helpers";
import { SETTINGS } from "@fig/api-bindings/wrappers";
import { type ArgumentParserResult, TokenType } from "@fig/autocomplete-parser";
import { SuggestionFlag } from "@fig/shared/utils";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { getFullHistorySuggestions } from "../../history";
import { useAutocompleteStore } from "..";

vi.mock("../../history", async (importOriginal) => {
  const original = await importOriginal<typeof import("../../history")>();
  return { ...original, getFullHistorySuggestions: vi.fn(original.getFullHistorySuggestions) };
});

const spec = convertSubcommand(
  { name: "tool", subcommands: [{ name: "build" }, { name: "clean" }] },
  initializeDefault,
);

const parserResult: ArgumentParserResult = {
  completionObj: spec,
  currentArg: null,
  passedOptions: [],
  searchTerm: "",
  commandIndex: 0,
  suggestionFlags: SuggestionFlag.Any,
  annotations: [{ text: "", type: TokenType.None }],
};

const type = (buffer: string) => {
  const { setFigState, setParserResult } = useAutocompleteStore.getState();
  setFigState((figState) => ({ ...figState, buffer, cursorLocation: buffer.length, cwd: "/" }));
  setParserResult({ ...parserResult }, false, false);
};

describe("store: history scan", () => {
  const initial = useAutocompleteStore.getState();

  beforeEach(() => {
    useAutocompleteStore.setState(initial, true);
    vi.mocked(getFullHistorySuggestions).mockClear();
  });

  it("does not scan history when history mode is off", () => {
    useAutocompleteStore.getState().setSettings({ [SETTINGS.HISTORY_MODE]: "off" });
    type("tool ");
    type("tool b");
    expect(useAutocompleteStore.getState().suggestions.length).toBeGreaterThan(0);
    expect(getFullHistorySuggestions).not.toHaveBeenCalled();
  });

  it.each(["show", "history_only"])("scans history when history mode is %s", (mode) => {
    useAutocompleteStore.getState().setSettings({ [SETTINGS.HISTORY_MODE]: mode });
    type("tool ");
    expect(getFullHistorySuggestions).toHaveBeenCalled();
  });

  it("scans history when history mode was toggled on by keybinding", () => {
    useAutocompleteStore.getState().setHistoryModeEnabled(true);
    type("tool ");
    expect(getFullHistorySuggestions).toHaveBeenCalled();
  });
});
