import { expect, it } from "vitest";
import { createCompletionCaches } from "./cacheScopes";
import { SETTINGS } from "@fig/shared/settings";
const execution = { sessionId: "one", cwd: "/repo", env: { PATH: "/usr/bin", HOME: "/home" } };
it("shares the catalog, isolates execution facts, and ignores appearance changes", () => {
  const caches = createCompletionCaches();
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
