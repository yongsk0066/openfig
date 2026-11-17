import { runtimeTiming } from "@fig/shared/runtime-timing";
import { expect, it, vi } from "vitest";
import { initialParserState } from "@fig/autocomplete-parser/core";
import { readSettings } from "@fig/shared/settings";
import { SpecLocationSource } from "@fig/shared/utils";
import { createHistoryRepository, type HistoryPorts } from "./repository";
const row = (id: number) => ({
  id,
  command: `git branch branch-${id}`,
  shell: "zsh",
  cwd: "/repo",
});
function fixture(count = 0) {
  const rows = Array.from({ length: count }, (_, i) => row(i + 1));
  const ports: HistoryPorts = {
    timing: runtimeTiming,
    getSetting: readSettings({}),
    query: vi.fn(async (sql, params) => {
      if (sql.includes("MAX(id)")) return [{ id: rows.at(-1)?.id ?? 0 }];
      const id = Number(params[0]);
      return sql.includes("id <")
        ? rows
            .filter((r) => r.id < id)
            .toReversed()
            .slice(0, 500)
        : rows.filter((r) => r.id > id).slice(0, 500);
    }),
    getSpecPath: vi.fn(async (name) => ({ name, type: SpecLocationSource.GLOBAL })),
    loadSubcommandCached: vi
      .fn<HistoryPorts["loadSubcommandCached"]>()
      .mockRejectedValue(new Error("fixture")),
    parseArguments: vi.fn(async () => initialParserState),
    executeCommand: vi.fn(async () => ({ stdout: "", stderr: "", status: 0 })),
    executeLoginShell: vi.fn(async () => ""),
  };
  return { rows, ports, repository: createHistoryRepository(ports) };
}
it("pre-aborted history does no query, spec loading, parsing or shell work", async () => {
  const { ports, repository } = fixture(600);
  const abort = new AbortController();
  abort.abort();
  await expect(repository.loadHistory({}, abort.signal)).rejects.toMatchObject({
    name: "AbortError",
  });
  for (const name of [
    "query",
    "parseArguments",
    "getSpecPath",
    "loadSubcommandCached",
    "executeLoginShell",
  ] as const)
    expect(ports[name]).not.toHaveBeenCalled();
});
it("publishes newest batch first, preserves final order and catches commits during startup without duplicates", async () => {
  const { rows, repository } = fixture(600);
  const publications: number[][] = [];
  await repository.loadHistory({}, undefined, () => {
    publications.push(
      repository
        .getHistorySources()
        .fig.entries.map((entry) => Number(entry.text.split("-").at(-1))),
    );
    if (rows.length === 600) rows.push(row(601));
  });
  expect(publications[0]).toEqual(Array.from({ length: 500 }, (_, i) => i + 101));
  expect(repository.getHistorySources().fig.entries).toHaveLength(601);
  expect(repository.getHistorySources().zsh.entries.at(-1)?.text).toBe(row(601).command);
  rows.push(row(602));
  await Promise.all([repository.refreshHistory({}), repository.refreshHistory({})]);
  expect(repository.getHistorySources().fig.entries.map((entry) => entry.text)).toEqual(
    rows.map((r) => r.command),
  );
});
it("aborts between query and parse without publishing and can subsequently restart", async () => {
  const { ports, repository } = fixture(3);
  const controller = new AbortController();
  const query = ports.query;
  ports.query = vi.fn(async (sql: string, params: string[]) => {
    const rows = await query(sql, params);
    controller.abort();
    return rows;
  });
  await expect(repository.loadHistory({}, controller.signal)).rejects.toMatchObject({
    name: "AbortError",
  });
  expect(ports.parseArguments).not.toHaveBeenCalled();
  expect(repository.getHistorySources().fig.entries).toEqual([]);
  ports.query = query;
  await repository.loadHistory({});
  expect(repository.getHistorySources().fig.entries).toHaveLength(3);
});
it("does not reuse annotations across working directories", async () => {
  const { repository, ports } = fixture();
  await repository.loadHistorySource(
    [
      { text: "git branch", commands: [], context: { cwd: "/one" } },
      { text: "git branch", commands: [], context: { cwd: "/two" } },
    ],
    {},
  );
  expect(ports.parseArguments).toHaveBeenCalledTimes(2);
  expect(
    vi.mocked(ports.parseArguments).mock.calls.map((call) => call[1].currentWorkingDirectory),
  ).toEqual(["/one", "/two"]);
});

it("tracks inline loadSpec boundaries through real parsing, history indexing and lookup", async () => {
  const { createParser } = await import("@fig/autocomplete-parser/core");
  const { convertSubcommand, initializeDefault } = await import("@fig/spec-helpers");
  const { getCommand } = await import("@fig/shell-parser");
  const { getHistoryArgSuggestions } = await import("./repository");
  const { ports } = fixture();
  const spec = convertSubcommand(
    {
      name: "tool",
      args: {
        name: "mode",
        loadSpec: async (mode) => ({
          name: mode,
          args: { name: `${mode}-value`, template: "history", isVariadic: true },
          options: [{ name: "--tag", args: { name: "tag", template: "history" } }],
          subcommands: [
            { name: "nested", loadSpec: { name: "inner", args: { name: "inner-value" } } },
          ],
        }),
      },
    },
    initializeDefault,
  );
  ports.loadSubcommandCached = async () => spec;
  const parser = createParser({
    withTimeout: runtimeTiming.timeout,
    specs: ports,
    executeCommand: ports.executeCommand,
    executeLoginShell: ports.executeLoginShell,
    getSetting: ports.getSetting,
    isInDevMode: () => false,
  });
  ports.parseArguments = parser.parseArguments;
  const repository = createHistoryRepository(ports);
  const source = await repository.loadHistorySource(
    [
      { text: "tool alpha first", context: { cwd: "/repo" }, commands: [] },
      { text: "tool alpha one two three", context: { cwd: "/repo" }, commands: [] },
      { text: "tool beta second", context: { cwd: "/repo" }, commands: [] },
      { text: "tool alpha --tag tagged first", context: { cwd: "/repo" }, commands: [] },
      { text: "tool alpha nested deep", context: { cwd: "/repo" }, commands: [] },
      { text: "tool alpha elsewhere", context: { cwd: "/elsewhere" }, commands: [] },
    ],
    {},
  );
  const lookup = async (text: string) => {
    const result = await parser.parseArguments(getCommand(text, {}), {
      currentWorkingDirectory: "/repo",
      currentProcess: "zsh",
      sshPrefix: "",
      environmentVariables: {},
    });
    return getHistoryArgSuggestions(result.annotations, "zsh", { fig: source }, {}).map(
      (item) => item.name,
    );
  };
  const context = {
    currentWorkingDirectory: "/repo",
    currentProcess: "zsh",
    sshPrefix: "",
    environmentVariables: {},
  };
  const quoted = await parser.parseArguments(getCommand("tool 'alpha beta' v", {}), context);
  const split = await parser.parseArguments(getCommand("tool alpha beta v", {}), context);
  expect(quoted.completionObj.name).toEqual(["alpha beta"]);
  expect(split.completionObj.name).toEqual(["alpha"]);
  expect(await lookup("tool alpha ")).toContain("first");
  expect(await lookup("tool alpha ")).not.toContain("second");
  expect(await lookup("tool alpha ")).not.toContain("elsewhere");
  expect(await lookup("tool beta ")).toEqual(["second"]);
  expect(await lookup("tool alpha one ")).toEqual(expect.arrayContaining(["one", "two", "three"]));
  expect(await lookup("tool alpha --tag ")).toEqual(["tagged"]);
  expect(await lookup("tool alpha nested ")).toEqual(["deep"]);
  expect(await lookup("tool ")).toEqual(expect.arrayContaining(["alpha", "beta"]));
});
