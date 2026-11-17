import { afterEach, expect, it, vi } from "vitest";
import { longestCommonPrefix } from "./utils";
import { withTimeout } from "./runtime-timing";
afterEach(() => vi.useRealTimers());
it("releases timeout and abort listeners on every settlement path", async () => {
  vi.useFakeTimers();
  const scope = new AbortController();
  const remove = vi.spyOn(scope.signal, "removeEventListener");
  await expect(withTimeout(5000, Promise.resolve(42), scope.signal)).resolves.toBe(42);
  await expect(
    withTimeout(5000, Promise.reject(new Error("failed")), scope.signal),
  ).rejects.toThrow("failed");
  const pending = withTimeout(5000, new Promise(() => {}), scope.signal);
  const rejected = expect(pending).rejects.toThrow("cancelled");
  scope.abort(new Error("cancelled"));
  await rejected;
  expect(vi.getTimerCount()).toBe(0);
  expect(remove).toHaveBeenCalledTimes(3);
  const timeout = expect(withTimeout(10, new Promise(() => {}))).rejects.toThrow("timed out");
  await vi.advanceTimersByTimeAsync(10);
  await timeout;
  expect(vi.getTimerCount()).toBe(0);
});
it("linear prefix preserves UTF-16 comparison and never mutates input", () => {
  for (const values of [
    [],
    ["one"],
    ["git α", "git β"],
    ["😀alpha", "😀alpine"],
    ["a", ""],
    ["Case", "case"],
  ]) {
    const sorted = values.toSorted();
    const first = sorted[0] ?? "",
      last = sorted.at(-1) ?? "";
    let i = 0;
    while (i < first.length && first[i] === last[i]) i++;
    const original = [...values];
    expect(longestCommonPrefix(values)).toBe(first.slice(0, i));
    expect(values).toEqual(original);
  }
});
