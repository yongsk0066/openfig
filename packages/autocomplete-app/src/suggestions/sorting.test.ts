import { runtimeTiming } from "@fig/shared/runtime-timing";
import { afterEach, expect, it, vi } from "vitest";
import { createRecencyRepository } from "./sorting";
afterEach(() => vi.useRealTimers());
it("keeps immutable ranking snapshots and coalesces persistence, including explicit disposal flush", () => {
  vi.useFakeTimers();
  const storage = { getItem: vi.fn(() => "{}"), setItem: vi.fn() };
  const repo = createRecencyRepository(storage, () => 42, runtimeTiming.schedule);
  repo.updateAutocompleteIndexFromUserInsert("git", "one");
  const snapshot = repo.getRecencyIndex();
  repo.updateAutocompleteIndexFromUserInsert("git", "two");
  expect(snapshot.git).toEqual({ one: 42 });
  expect(storage.setItem).not.toHaveBeenCalled();
  vi.advanceTimersByTime(250);
  expect(storage.setItem).toHaveBeenCalledExactlyOnceWith(
    "term_frequency_index_fig_only_recency",
    JSON.stringify({ git: { one: 42, two: 42 } }),
  );
  repo.updateAutocompleteIndexFromUserInsert("git", "three");
  repo.flush();
  expect(storage.setItem).toHaveBeenCalledTimes(2);
  expect(vi.getTimerCount()).toBe(0);
});
it("loads only valid recent entries, bounds each command and retries failed persistence", () => {
  vi.useFakeTimers();
  const values = Object.fromEntries(Array.from({ length: 2000 }, (_, i) => [`branch-${i}`, i]));
  const storage = {
    getItem: () => JSON.stringify({ git: values, bad: null, other: { invalid: "number" } }),
    setItem: vi.fn().mockImplementationOnce(() => {
      throw new Error("full");
    }),
  };
  const repo = createRecencyRepository(storage, () => 3000, runtimeTiming.schedule);
  expect(Object.keys(repo.getRecencyIndex().git)).toHaveLength(1000);
  expect(repo.getRecencyIndex().git["branch-1999"]).toBe(1999);
  expect(repo.getRecencyIndex().other).toBeUndefined();
  repo.updateAutocompleteIndexFromUserInsert("git", "new");
  repo.flush();
  repo.flush();
  expect(storage.setItem).toHaveBeenCalledTimes(2);
  expect(Object.keys(repo.getRecencyIndex().git)).toHaveLength(1000);
});
