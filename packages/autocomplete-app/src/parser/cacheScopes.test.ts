import { expect, it } from "vitest";
import { createCompletionCaches } from "./cacheScopes";
import { SETTINGS } from "@fig/shared/settings";
const execution = { sessionId: "one", cwd: "/repo", env: { PATH: "/usr/bin", HOME: "/home" } };
it("shares the catalog, isolates execution facts, and ignores appearance changes", () => {
  const caches = createCompletionCaches({ now: () => 0 });
  const first = caches.scope(execution, {});
  const appearance = caches.scope(execution, { "autocomplete.fontSize": 18 });
  expect(appearance.parser).toBe(first.parser);
  expect(appearance.loader).toBe(first.loader);
  const parseSetting = caches.scope(execution, { [SETTINGS.FIRST_COMMAND_COMPLETION]: false });
  expect(parseSetting.parser).not.toBe(first.parser);
  expect(parseSetting.loader).toBe(first.loader);
  for (const context of [
    { ...execution, cwd: "/other" },
    { ...execution, sessionId: "two" },
    { ...execution, env: { ...execution.env, PATH: "/another/bin" } },
  ]) {
    const other = caches.scope(context, {});
    expect(other.loader).not.toBe(first.loader);
    expect(other.loader.catalog).toBe(first.loader.catalog);
  }
  expect(caches.scope({ ...execution, env: { HOME: "/home", PATH: "/usr/bin" } }, {}).loader).toBe(
    first.loader,
  );
  caches.clear();
  expect(caches.scope(execution, {}).loader).not.toBe(first.loader);
});

it("expires execution facts and dependent parsed specs together without sliding the deadline", () => {
  let time = 0;
  const caches = createCompletionCaches({ now: () => time, maxAgeMs: 100 });
  const first = caches.scope(execution, {});
  time = 99;
  expect(caches.scope(execution, {}).loader).toBe(first.loader);
  time = 100;
  const refreshed = caches.scope(execution, {});
  expect(refreshed.loader).not.toBe(first.loader);
  expect(refreshed.parser).not.toBe(first.parser);
  expect(refreshed.specs).not.toBe(first.specs);
  expect(refreshed.loader.catalog).toBe(first.loader.catalog);
  // A parser-setting variant cannot resurrect the previous execution generation.
  expect(caches.scope(execution, { [SETTINGS.FIRST_COMMAND_COMPLETION]: false }).loader).toBe(
    refreshed.loader,
  );
});

it("does not revive parsed specs when an execution scope has been evicted", () => {
  const caches = createCompletionCaches({ now: () => 0 });
  const first = caches.scope(execution, {});
  for (let i = 0; i < 24; i++) {
    caches.scope({ ...execution, sessionId: String(i) }, {});
  }
  const revisited = caches.scope(execution, {});
  expect(revisited.loader).not.toBe(first.loader);
  expect(revisited.parser).not.toBe(first.parser);
});

it("rejects an unbounded or non-positive lifetime", () => {
  for (const maxAgeMs of [0, -1, Infinity, NaN]) {
    expect(() => createCompletionCaches({ now: () => 0, maxAgeMs })).toThrow(RangeError);
  }
});
