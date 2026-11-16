import { convertSubcommand, initializeDefault } from "@fig/spec-helpers";
import { SuggestionFlag, SpecLocationSource } from "@fig/shared/utils";
import { getCommand } from "@fig/shell-parser";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { resetCaches } from "../src/index";
import { loadSubcommandCached } from "../src/loadSpec";
import { parseArguments, TokenType } from "../src/parseArguments";

vi.mock("../src/loadSpec", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../src/loadSpec")>()),
  loadSubcommandCached: vi.fn(),
}));

// `git`'s generateSpec runs `git help -a`; an empty answer leaves the static spec.
vi.mock("@fig/api-bindings/wrappers", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@fig/api-bindings/wrappers")>()),
  executeCommand: vi.fn(async () => ({ status: 0, stdout: "", stderr: "" })),
}));

/**
 * The shipped spec sources, loaded at run time. A static import would make this library
 * package depend on `specs/` (tagged `spec`), which `turbo boundaries` forbids for `lib`.
 */
const loadShippedSpec = async (name: string): Promise<Fig.Spec> => {
  const url = new URL(`../../../specs/src/${name}.ts`, import.meta.url);
  const module = (await import(/* @vite-ignore */ url.href)) as { default: Fig.Spec };
  return module.default;
};

const specs: Record<string, Fig.Spec> = {};

const context: Fig.ShellContext = {
  currentWorkingDirectory: "/repo",
  currentProcess: "/bin/zsh",
  sshPrefix: "",
  environmentVariables: {},
};

const parse = (buffer: string, aliases: Record<string, string> = {}) =>
  parseArguments(getCommand(buffer, aliases), context);

const names = (objects: { name: string[] }[]) => objects.map((o) => o.name);

describe("parseArguments with the shipped git, cd and npm specs", () => {
  beforeAll(async () => {
    globalThis.fig = { ...globalThis.fig, settings: {} };
    for (const name of ["cd", "git", "npm"]) {
      specs[name] = await loadShippedSpec(name);
    }
  });

  beforeEach(() => {
    resetCaches();
    vi.mocked(loadSubcommandCached).mockReset();
    vi.mocked(loadSubcommandCached).mockImplementation(async (location) =>
      convertSubcommand(specs[location.name] as Fig.Subcommand, initializeDefault),
    );
  });

  it("git ch: completing a subcommand of git", async () => {
    const result = await parse("git ch");
    expect(loadSubcommandCached).toHaveBeenCalledWith(
      { name: "git", type: SpecLocationSource.GLOBAL },
      context,
      expect.anything(),
    );
    expect(result.completionObj.name).toEqual(["git"]);
    expect(result.searchTerm).toBe("ch");
    expect(result.suggestionFlags).toBe(SuggestionFlag.Any);
    expect(result.completionObj.subcommands.checkout.name).toEqual(["checkout"]);
    expect(result.completionObj.subcommands["cherry-pick"].name).toEqual(["cherry-pick"]);
    expect(result.annotations.map((a) => [a.type, a.text])).toEqual([
      [TokenType.Subcommand, "git"],
      [TokenType.SubcommandArg, "ch"],
    ]);
  });

  it("git checkout <space>: the branch argument with its generators", async () => {
    const result = await parse("git checkout ");
    expect(result.completionObj.name).toEqual(["checkout"]);
    expect(result.searchTerm).toBe("");
    expect(result.currentArg?.name).toBe("branch, file, tag or commit");
    expect(result.currentArg?.filterStrategy).toBe("fuzzy");
    expect(result.currentArg?.generators).toHaveLength(3);
  });

  it("git commit --am: an option being typed", async () => {
    const result = await parse("git commit --am");
    expect(result.completionObj.name).toEqual(["commit"]);
    expect(result.searchTerm).toBe("--am");
    expect(result.suggestionFlags & SuggestionFlag.Options).toBeTruthy();
    expect(result.completionObj.options["--amend"].name).toEqual(["--amend"]);
  });

  it("git commit -m <space>: only the option's argument", async () => {
    const result = await parse("git commit -m ");
    expect(result.suggestionFlags).toBe(SuggestionFlag.Args);
    expect(result.currentArg?.name).toBe("message");
    expect(names(result.passedOptions)).toEqual([["-m", "--message"]]);
    expect(result.annotations.map((a) => a.type)).toEqual([
      TokenType.Subcommand,
      TokenType.Subcommand,
      TokenType.Composite,
      TokenType.OptionArg,
    ]);
  });

  it("g ch with alias g=git parses as git ch", async () => {
    const aliased = await parse("g ch", { g: "git" });
    resetCaches();
    const plain = await parse("git ch");
    expect(aliased.completionObj.name).toEqual(plain.completionObj.name);
    expect(aliased.searchTerm).toBe("ch");
    expect(aliased.suggestionFlags).toBe(plain.suggestionFlags);
  });

  it("cd pro: a folder argument with the filepaths generator, fuzzy filtered", async () => {
    const result = await parse("cd pro");
    expect(result.completionObj.name).toEqual(["cd"]);
    expect(result.searchTerm).toBe("pro");
    expect(result.currentArg?.filterStrategy).toBe("fuzzy");
    expect(result.currentArg?.generators).toHaveLength(1);
    expect(result.currentArg?.suggestions).toHaveLength(2);
  });

  it("npm i -D typescript <space>: a passed option and a variadic argument", async () => {
    const result = await parse("npm i -D typescript ");
    expect(result.completionObj.name).toEqual(["install", "i", "add"]);
    expect(names(result.passedOptions)).toEqual([["-D", "--save-dev"]]);
    expect(result.currentArg?.name).toBe("package");
    // An argument has been consumed, so subcommands are no longer offered.
    expect(result.suggestionFlags).toBe(SuggestionFlag.Args | SuggestionFlag.Options);
  });

  it("npm run <space>: the script argument", async () => {
    const result = await parse("npm run ");
    expect(result.completionObj.name).toEqual(["run", "run-script"]);
    expect(result.currentArg?.name).toBe("script");
    expect(result.currentArg?.filterStrategy).toBe("fuzzy");
  });

  it("caches a parse by command prefix", async () => {
    await parse("git checkout ma");
    await parse("git checkout mai");
    expect(loadSubcommandCached).toHaveBeenCalledTimes(1);
  });
});
