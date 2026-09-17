import type { Suggestion } from "@fig/shared/internal";
import { SETTINGS, type SettingsMap } from "@fig/api-bindings/wrappers";
import {
  type ArgumentParserResult,
  initialParserState,
  loadSubcommandCached,
  parseArguments,
} from "@fig/autocomplete-parser";
import { convertSubcommand, initializeDefault } from "@fig/spec-helpers";
import { getCommand } from "@fig/shell-parser";
import { beforeAll, describe, expect, it, vi } from "vitest";
import type { GeneratorState } from "../../generators/helpers";
import type { HistorySources } from "../../history";
import { computeSuggestions, type SuggestionsInput } from "../pipeline";

// Parse with the real parser; only spec loading (a `spec://` import in the app) and shell
// commands (`git`'s generateSpec runs `git help -a`) are replaced.
vi.mock("../../../../autocomplete-parser/src/loadSpec.ts", async (importOriginal) => ({
  ...(await importOriginal<Record<string, unknown>>()),
  loadSubcommandCached: vi.fn(),
}));
vi.mock("@fig/api-bindings/wrappers", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@fig/api-bindings/wrappers")>()),
  executeCommand: vi.fn(async () => ({ status: 0, stdout: "", stderr: "" })),
}));

/**
 * The shipped spec sources, loaded at run time: a static import would make this app package
 * depend on `specs/` (tagged `spec`), which `turbo boundaries` forbids.
 */
const loadShippedSpec = async (name: string): Promise<Fig.Spec> => {
  const url = new URL(`../../../../../specs/src/${name}.ts`, import.meta.url);
  const module = (await import(/* @vite-ignore */ url.href)) as { default: Fig.Spec };
  return module.default;
};

const context: Fig.ShellContext = {
  currentWorkingDirectory: "/repo",
  currentProcess: "/bin/zsh",
  sshPrefix: "",
  environmentVariables: {},
};

const noHistory: HistorySources = {
  zsh: { entries: [] },
  bash: { entries: [] },
  fig: { entries: [] },
  customCommand: { entries: [] },
};

const zshHistory: HistorySources = {
  ...noHistory,
  zsh: {
    entries: ["git checkout main", "git cherry-pick abc123", "git commit -m wip", "ls -la"].map(
      (text) => ({ text, commands: [] }),
    ),
  },
};

type Fixture = {
  buffer: string;
  aliases?: Record<string, string>;
  parserResult: ArgumentParserResult;
};

const parse = async (buffer: string, aliases: Record<string, string> = {}): Promise<Fixture> => ({
  buffer,
  aliases,
  parserResult: await parseArguments(getCommand(buffer, aliases), context),
});

const input = (fixture: Fixture, overrides: Partial<SuggestionsInput> = {}): SuggestionsInput => ({
  buffer: fixture.buffer,
  command: getCommand(fixture.buffer, fixture.aliases ?? {}),
  context: { currentProcess: context.currentProcess },
  parserResult: fixture.parserResult,
  generatorStates: [],
  settings: {},
  history: { modeEnabled: false, sources: noHistory, recency: {} },
  fuzzy: false,
  selection: { suggestions: [], selectedIndex: 0, hasChangedIndex: false, visible: true },
  ...overrides,
});

// `git commit` was inserted last (recency index: command → argument → timestamp).
const recency = { git: { commit: 1_700_000_000_000 } };

const firstName = (s: Suggestion) => (Array.isArray(s.name) ? s.name[0] : s.name);
const rows = (suggestions: Suggestion[]) => suggestions.map((s) => [s.name, s.type]);
const names = (suggestions: Suggestion[]) => suggestions.map((s) => s.name);

/** Every object reachable from `value`, frozen, so any write in the pipeline throws. */
const deepFreeze = <T>(value: T, seen = new Set<unknown>()): T => {
  if (value && (typeof value === "object" || typeof value === "function") && !seen.has(value)) {
    seen.add(value);
    Object.values(value).forEach((child) => deepFreeze(child, seen));
    Object.freeze(value);
  }
  return value;
};

/** A history index that fails the test if the pipeline so much as looks at it. */
const untouchable = new Proxy({} as HistorySources, {
  get: (_target, key) => {
    throw new Error(`history read: ${String(key)}`);
  },
  has: (_target, key) => {
    throw new Error(`history read: ${String(key)}`);
  },
});

describe("computeSuggestions", () => {
  const fixtures: Record<string, Fixture> = {};

  beforeAll(async () => {
    const specs: Record<string, Fig.Spec> = {
      git: await loadShippedSpec("git"),
      cd: await loadShippedSpec("cd"),
    };
    vi.mocked(loadSubcommandCached).mockImplementation(async (location) =>
      convertSubcommand(specs[location.name] as Fig.Subcommand, initializeDefault),
    );
    for (const buffer of ["git ch", "git c", "git cko", "git checkout", "git commit --am", "gi"]) {
      fixtures[buffer] = await parse(buffer);
    }
    fixtures["cd pro"] = await parse("cd pro");
    fixtures["ls && "] = await parse("ls && ");
    fixtures["g ch"] = await parse("g ch", { g: "git" });
  });

  it("subcommand: `git ch` offers the subcommands starting with ch", () => {
    const output = computeSuggestions(input(fixtures["git ch"]));
    expect(rows(output.suggestions)).toEqual([
      [["checkout"], "subcommand"],
      [["cherry-pick"], "subcommand"],
    ]);
    expect(output).toMatchObject({ selectedIndex: 0, hasChangedIndex: false });
  });

  it("option: `git commit --am` offers --amend", () => {
    const { suggestions } = computeSuggestions(input(fixtures["git commit --am"]));
    expect(rows(suggestions)).toEqual([[["--amend"], "option"]]);
  });

  it("filepath argument: `cd pro` filters the folders generator's result", () => {
    const fixture = fixtures["cd pro"];
    const generatorState: GeneratorState = {
      generator: fixture.parserResult.currentArg!.generators[0],
      context: { ...context, annotations: [], tokenArray: ["cd", "pro"], searchTerm: "pro" },
      loading: false,
      result: ["projects/", "prod/", "src/", "Pictures/"].map((name) => ({
        name,
        type: "folder",
        context: { templateType: "folders" },
      })),
    };
    const { suggestions } = computeSuggestions(
      input(fixture, { generatorStates: [generatorState], fuzzy: true }),
    );
    expect(rows(suggestions)).toEqual([
      ["projects/", "folder"],
      ["prod/", "folder"],
    ]);

    // While a generator that filters by query term is loading, its old result is withheld.
    const loading = computeSuggestions(
      input(fixture, { generatorStates: [{ ...generatorState, loading: true }], fuzzy: true }),
    );
    expect(loading.suggestions).toEqual([]);
  });

  it("alias-expanded: `g ch` with g=git gives what `git ch` gives", () => {
    const aliased = computeSuggestions(input(fixtures["g ch"]));
    const plain = computeSuggestions(input(fixtures["git ch"]));
    expect(input(fixtures["g ch"]).command?.tokens.map((t) => t.text)).toEqual(["git", "ch"]);
    expect(aliased).toEqual(plain);
  });

  describe("history mode", () => {
    it("show: history entries after the spec suggestions", () => {
      const { suggestions } = computeSuggestions(
        input(fixtures["git ch"], {
          settings: { [SETTINGS.HISTORY_MODE]: "show" },
          history: { modeEnabled: false, sources: zshHistory, recency: {} },
        }),
      );
      expect(rows(suggestions)).toEqual([
        [["checkout"], "subcommand"],
        [["cherry-pick"], "subcommand"],
        ["checkout main", "history"],
        ["cherry-pick abc123", "history"],
      ]);
    });

    it("history_only and the history keybinding: only history entries", () => {
      const expected = [
        ["checkout main", "history"],
        ["cherry-pick abc123", "history"],
      ];
      const historyOnly = computeSuggestions(
        input(fixtures["git ch"], {
          settings: { [SETTINGS.HISTORY_MODE]: "history_only" },
          history: { modeEnabled: false, sources: zshHistory, recency: {} },
        }),
      );
      const keybinding = computeSuggestions(
        input(fixtures["git ch"], {
          history: { modeEnabled: true, sources: zshHistory, recency: {} },
        }),
      );
      expect(rows(historyOnly.suggestions)).toEqual(expected);
      expect(rows(keybinding.suggestions)).toEqual(expected);
    });

    it("off: the history index is never read", () => {
      for (const settings of [{}, { [SETTINGS.HISTORY_MODE]: "off" }] as SettingsMap[]) {
        const { suggestions } = computeSuggestions(
          input(fixtures["git ch"], {
            settings,
            history: { modeEnabled: false, sources: untouchable, recency: {} },
          }),
        );
        expect(names(suggestions)).toEqual([["checkout"], ["cherry-pick"]]);
      }
    });
  });

  it("fuzzy on/off: `git cko` matches checkout only when fuzzy", () => {
    const fixture = fixtures["git cko"];
    expect(names(computeSuggestions(input(fixture, { fuzzy: true })).suggestions)).toEqual([
      ["checkout"],
    ]);
    expect(computeSuggestions(input(fixture, { fuzzy: false })).suggestions).toEqual([]);
  });

  it("no annotations: the store's initial parse and an empty command after `&&`", () => {
    const empty = { suggestions: [], selectedIndex: 0, hasChangedIndex: false };
    // What the store holds at startup and after `error()`, which SHOW_AUTOCOMPLETE can show.
    expect(initialParserState.annotations).toEqual([]);
    expect(computeSuggestions(input({ buffer: "", parserResult: initialParserState }))).toEqual(
      empty,
    );
    // The parser annotates no token for an empty first word.
    expect(fixtures["ls && "].parserResult.annotations).toEqual([]);
    expect(computeSuggestions(input(fixtures["ls && "]))).toEqual(empty);
  });

  it("empty: a first word being typed has nothing to suggest", () => {
    // `gi` parses against the first-token spec, which has no arguments by default.
    expect(computeSuggestions(input(fixtures.gi))).toEqual({
      suggestions: [],
      selectedIndex: 0,
      hasChangedIndex: false,
    });
  });

  describe("settings change", () => {
    it("sortMethod: recency puts the last inserted subcommand first, alphabetical does not", () => {
      const fixture = fixtures["git c"];
      const history = { modeEnabled: false, sources: noHistory, recency };
      const byRecency = computeSuggestions(input(fixture, { history }));
      const alphabetical = computeSuggestions(
        input(fixture, { history, settings: { [SETTINGS.SORT_METHOD]: "alphabetical" } }),
      );
      expect(names(byRecency.suggestions).slice(0, 3)).toEqual([
        ["commit"],
        ["checkout"],
        ["cherry-pick"],
      ]);
      expect(names(alphabetical.suggestions).slice(0, 6)).toEqual([
        ["checkout"],
        ["cherry-pick"],
        ["clean"],
        ["clone"],
        ["commit"],
        ["config"],
      ]);
    });

    it("hideAutoExecuteSuggestion drops the auto-execute row of an exact match", () => {
      const fixture = fixtures["git checkout"];
      expect(rows(computeSuggestions(input(fixture)).suggestions)).toEqual([
        [["checkout"], "auto-execute"],
        [["checkout"], "subcommand"],
      ]);
      const hidden = computeSuggestions(
        input(fixture, { settings: { [SETTINGS.HIDE_AUTO_EXECUTE_SUGGESTION]: true } }),
      );
      expect(rows(hidden.suggestions)).toEqual([[["checkout"], "subcommand"]]);
    });
  });

  it("a selection the user moved follows its item into the new list", () => {
    const previous = computeSuggestions(input(fixtures["git c"])).suggestions;
    const selectedIndex = previous.findIndex((s) => firstName(s) === "cherry-pick");
    const next = computeSuggestions(
      input(fixtures["git ch"], {
        selection: { suggestions: previous, selectedIndex, hasChangedIndex: true, visible: true },
      }),
    );
    expect(next).toMatchObject({ selectedIndex: 1, hasChangedIndex: true });

    const hidden = computeSuggestions(
      input(fixtures["git ch"], {
        selection: { suggestions: previous, selectedIndex, hasChangedIndex: true, visible: false },
      }),
    );
    expect(hidden).toMatchObject({ selectedIndex: 0, hasChangedIndex: false });
  });

  it("is pure: frozen inputs, equal outputs on repeated calls", () => {
    const frozen = deepFreeze(
      input(fixtures["git ch"], {
        settings: { [SETTINGS.HISTORY_MODE]: "show" },
        history: { modeEnabled: false, sources: zshHistory, recency },
      }),
    );
    const first = computeSuggestions(frozen);
    const second = computeSuggestions(frozen);
    expect(second).toEqual(first);
    expect(first.suggestions).toHaveLength(4);
  });
});
