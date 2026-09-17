import { Keybindings } from "@fig/api-bindings";
import { type ArgumentParserResult, TokenType } from "@fig/autocomplete-parser";
import { convertSubcommand, initializeDefault } from "@fig/spec-helpers";
import type { Suggestion } from "@fig/shared/internal";
import { SuggestionFlag } from "@fig/shared/utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { initialFigState } from "../../fig/hooks";
import { useAutocompleteStore } from "..";
import { storeEffects } from "../effects";
import { figStateTransition, parserResultTransition } from "../transitions";
import { type AutocompleteState, Visibility } from "../types";

vi.mock("@fig/api-bindings", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@fig/api-bindings")>();
  return { ...actual, Keybindings: { ...actual.Keybindings, setInterceptKeystrokes: vi.fn() } };
});

const branches = vi.fn(async (): Promise<Fig.Suggestion[]> => [{ name: "main" }, { name: "dev" }]);

const spec = convertSubcommand(
  {
    name: "tool",
    subcommands: [
      { name: "switch", args: { name: "branch", generators: { custom: branches } } },
      { name: "search", args: { name: "query", debounce: true, generators: { custom: branches } } },
    ],
  },
  initializeDefault,
);

const parserResult = (subcommand: "switch" | "search", searchTerm = ""): ArgumentParserResult => {
  const completionObj = spec.subcommands[subcommand];
  return {
    completionObj,
    currentArg: completionObj.args[0],
    passedOptions: [],
    searchTerm,
    commandIndex: 0,
    suggestionFlags: SuggestionFlag.Args,
    annotations: [
      { text: "tool", type: TokenType.Subcommand },
      { text: subcommand, type: TokenType.Subcommand },
      { text: searchTerm, type: TokenType.SubcommandArg },
    ],
  };
};

const initial = useAutocompleteStore.getState();

const typed = (buffer: string): AutocompleteState => {
  useAutocompleteStore.getState().setFigState((figState) => ({
    ...figState,
    buffer,
    cursorLocation: buffer.length,
    cwd: "/repo",
    processUserIsIn: "/bin/zsh",
  }));
  return useAutocompleteStore.getState();
};

beforeEach(() => {
  useAutocompleteStore.setState(initial, true);
  branches.mockClear();
  vi.mocked(Keybindings.setInterceptKeystrokes).mockClear();
  window.globalCWD = "";
});

afterEach(() => {
  vi.useRealTimers();
});

const commandTokens = () => useAutocompleteStore.getState().command?.tokens.map((t) => t.text);
const withAlias = (alias: string) => ({ sessionId: "session-1", alias }) as never;

describe("transitions return effects instead of performing them", () => {
  it("a new argument plans its generator run without starting it", () => {
    const state = typed("tool switch ");
    const next = parserResultTransition(state, parserResult("switch"), false, false);

    expect(branches).not.toHaveBeenCalled();
    expect(next.effects).toEqual([
      { kind: "runGenerator", generatorState: next.state.generatorStates![0] },
    ]);
    expect(next.state.generatorStates![0]).toMatchObject({ loading: true, result: [] });
    expect(next.state.visibleState).toBe(Visibility.VISIBLE);
  });

  it("a debounced argument asks for its delay", () => {
    const next = parserResultTransition(
      typed("tool search "),
      parserResult("search"),
      false,
      false,
    );
    // `debounce: true` means the default delay.
    expect(next.effects).toMatchObject([{ kind: "runGenerator", debounceMs: 200 }]);
  });

  it("the same argument again plans nothing and keeps the generator states", () => {
    const before = typed("tool switch ");
    const state = {
      ...before,
      ...parserResultTransition(before, parserResult("switch"), false, false).state,
    };
    // Typing into the argument: no `trigger`, no debounce, so the generator does not rerun.
    const again = parserResultTransition(state, parserResult("switch", "ma"), false, false);
    expect(again.effects).toEqual([]);
    expect(again.state.generatorStates).toBe(state.generatorStates);
  });

  it("a buffer update returns the shell globals and leaves window and its input alone", () => {
    const state = useAutocompleteStore.getState();
    const figState = Object.freeze({
      ...initialFigState,
      buffer: "tool ",
      cursorLocation: 5,
      cwd: "/repo",
      sshContextString: "ssh host",
      shellContext: { sessionId: "session-1", alias: "t='tool'" } as never,
    });
    const next = figStateTransition(state, figState);

    expect(window.globalCWD).toBe("");
    expect(next.effects).toEqual([
      {
        kind: "setShellGlobals",
        globals: {
          globalCWD: "/repo",
          globalSSHString: "ssh host",
          globalTerminalSessionId: "session-1",
        },
      },
    ]);
    expect(next.state.figState).not.toBe(figState);
    expect(next.state.figState?.aliases).toEqual({ t: "tool" });
    expect(next.state.command?.tokens.map((t) => t.text)).toEqual(["tool", ""]);
  });

  it("an alias list expands the buffer of the update that carries it", () => {
    // A shell session sends the alias list only when it changed, together with a buffer.
    useAutocompleteStore.getState().setFigState((figState) => ({
      ...figState,
      buffer: "t ",
      cursorLocation: 2,
      shellContext: withAlias("t='tool'"),
    }));
    expect(useAutocompleteStore.getState().figState.aliases).toEqual({ t: "tool" });
    expect(commandTokens()).toEqual(["tool", ""]);

    // A new definition applies at once too, even if the buffer did not change.
    useAutocompleteStore.getState().setFigState((figState) => ({
      ...figState,
      shellContext: withAlias("t='tree'"),
    }));
    expect(commandTokens()).toEqual(["tree", ""]);
  });
});

const list = (n: number) => Array.from({ length: n }, (_, i): Suggestion => ({ name: `s${i}` }));

describe("storeEffects", () => {
  const at = (visibleState: Visibility, suggestions: Suggestion[]): AutocompleteState => ({
    ...initial,
    visibleState,
    suggestions,
    figState: { ...initialFigState, shellContext: { sessionId: "s" } as never },
  });

  it("intercepts keystrokes while a non-empty list is visible, on every change of either", () => {
    expect(
      storeEffects(at(Visibility.HIDDEN_UNTIL_KEYPRESS, []), at(Visibility.VISIBLE, list(3))),
    ).toEqual([
      { kind: "setInterceptKeystrokes", intercept: true, globalIntercept: true, sessionId: "s" },
    ]);
    expect(
      storeEffects(at(Visibility.VISIBLE, list(3)), at(Visibility.VISIBLE, list(2))),
    ).toHaveLength(1);
    expect(
      storeEffects(at(Visibility.VISIBLE, list(3)), at(Visibility.HIDDEN_UNTIL_SHOWN, list(3))),
    ).toEqual([
      { kind: "setInterceptKeystrokes", intercept: false, globalIntercept: true, sessionId: "s" },
    ]);
    expect(storeEffects(at(Visibility.VISIBLE, list(3)), at(Visibility.VISIBLE, list(3)))).toEqual(
      [],
    );
  });
});

describe("store: effects run after the state is written", () => {
  it("globals, then the generator, then its result", async () => {
    typed("tool switch ");
    expect(window.globalCWD).toBe("/repo");

    let loadingWhenStarted: boolean | undefined;
    branches.mockImplementationOnce(async () => {
      loadingWhenStarted = useAutocompleteStore.getState().generatorStates[0]?.loading;
      return [{ name: "main" }, { name: "dev" }];
    });
    useAutocompleteStore.getState().setParserResult(parserResult("switch"), false, false);

    expect(branches).toHaveBeenCalledTimes(1);
    expect(loadingWhenStarted).toBe(true);
    await vi.waitFor(() =>
      expect(useAutocompleteStore.getState().generatorStates[0].loading).toBe(false),
    );
    const { suggestions } = useAutocompleteStore.getState();
    expect(suggestions.map((s) => s.name)).toEqual(["main", "dev"]);
    expect(Keybindings.setInterceptKeystrokes).toHaveBeenLastCalledWith(
      expect.anything(),
      true,
      true,
      undefined,
    );
  });

  it("a debounced generator starts after the delay, and not at all if the argument changed", async () => {
    vi.useFakeTimers();
    typed("tool search ");
    useAutocompleteStore.getState().setParserResult(parserResult("search"), false, false);
    await vi.advanceTimersByTimeAsync(199);
    expect(branches).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(branches).toHaveBeenCalledTimes(1);

    branches.mockClear();
    useAutocompleteStore.getState().setParserResult(parserResult("switch"), false, false);
    expect(branches).toHaveBeenCalledTimes(1); // switch runs at once
    typed("tool search ");
    useAutocompleteStore.getState().setParserResult(parserResult("search"), false, false);
    useAutocompleteStore.getState().setParserResult(parserResult("switch"), false, false);
    await vi.advanceTimersByTimeAsync(500);
    // The superseded `search` run never started; only the two immediate `switch` runs did.
    expect(branches).toHaveBeenCalledTimes(2);
  });
});
