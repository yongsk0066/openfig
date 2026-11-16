import { afterEach, beforeEach, expect, it, vi } from "vitest";
import logger from "loglevel";
import { subscribeWithRetry } from "../subscription";

beforeEach(() => {
  vi.useFakeTimers();
  vi.spyOn(logger, "warn").mockImplementation(() => {});
});
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it("recovers after transient failures and stops retrying once connected", async () => {
  const unsubscribe = vi.fn();
  const getSubscription = vi
    .fn()
    .mockRejectedValueOnce(new Error("Load failed"))
    .mockRejectedValueOnce(new Error("desktop busy"))
    .mockResolvedValue({ unsubscribe });
  const stop = subscribeWithRetry(getSubscription);
  await vi.advanceTimersByTimeAsync(30_000);
  expect(getSubscription).toHaveBeenCalledTimes(3);
  expect(unsubscribe).not.toHaveBeenCalled();
  stop();
  expect(unsubscribe).toHaveBeenCalledTimes(1);
});

it("does not keep retrying an unmounted subscription", async () => {
  const getSubscription = vi.fn().mockRejectedValue(new Error("Load failed"));
  const stop = subscribeWithRetry(getSubscription);
  await vi.advanceTimersByTimeAsync(0);
  stop();
  await vi.advanceTimersByTimeAsync(60_000);
  expect(getSubscription).toHaveBeenCalledTimes(1);
});

it("cleans up a registration that finishes after unmount", async () => {
  let finish!: (value: { unsubscribe(): void }) => void;
  const unsubscribe = vi.fn();
  const stop = subscribeWithRetry(() => new Promise((resolve) => (finish = resolve)));
  stop();
  finish({ unsubscribe });
  await vi.advanceTimersByTimeAsync(0);
  expect(unsubscribe).toHaveBeenCalledTimes(1);
  expect(vi.getTimerCount()).toBe(0);
});
