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
