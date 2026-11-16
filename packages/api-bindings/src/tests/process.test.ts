import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { run } from "../process";
import { sendRunProcessRequest, sendCancelProcessRequest } from "../requests";
vi.mock("../requests", () => ({
  sendRunProcessRequest: vi.fn(),
  sendCancelProcessRequest: vi.fn(),
}));
const response = { stdout: "ok", stderr: "", exitCode: 0 };
beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(sendRunProcessRequest).mockResolvedValue(response as never);
  vi.mocked(sendCancelProcessRequest).mockResolvedValue(undefined as never);
});
afterEach(() => vi.useRealTimers());
it.each([
  [undefined, { secs: 60n, nanos: 0 }],
  [0, { secs: 0n, nanos: 0 }],
  [50, { secs: 0n, nanos: 50_000_000 }],
  [1500, { secs: 1n, nanos: 500_000_000 }],
  [5000, { secs: 5n, nanos: 0 }],
  [120000, { secs: 60n, nanos: 0 }],
])("preserves a bounded %s ms deadline on the wire", async (timeout, duration) => {
  await run({ executable: "git", args: [], timeout });
  expect(vi.mocked(sendRunProcessRequest).mock.calls[0][0].timeout).toMatchObject(duration);
});
it("does not start already cancelled work", async () => {
  const controller = new AbortController();
  controller.abort();
  await expect(
    run({ executable: "git", args: [], signal: controller.signal }),
  ).rejects.toMatchObject({ name: "AbortError" });
  expect(sendRunProcessRequest).not.toHaveBeenCalled();
});
it("cancels the native token and ignores a late response", async () => {
  let finish!: (value: never) => void;
  vi.mocked(sendRunProcessRequest).mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const controller = new AbortController();
  const promise = run({ executable: "git", args: [], signal: controller.signal });
  const check = expect(promise).rejects.toMatchObject({ name: "AbortError" });
  controller.abort();
  await check;
  expect(sendCancelProcessRequest).toHaveBeenCalledExactlyOnceWith({
    cancellationToken: vi.mocked(sendRunProcessRequest).mock.calls[0][0].cancellationToken,
  });
  finish(response as never);
  await Promise.resolve();
});
it("cancels a native request that never replies", async () => {
  vi.useFakeTimers();
  vi.mocked(sendRunProcessRequest).mockImplementation(() => new Promise(() => {}));
  const check = expect(run({ executable: "git", args: [], timeout: 50 })).rejects.toMatchObject({
    name: "TimeoutError",
  });
  await vi.advanceTimersByTimeAsync(1050);
  await check;
  expect(sendCancelProcessRequest).toHaveBeenCalledTimes(1);
});
it("cleans up cancellation after completion", async () => {
  vi.useFakeTimers();
  const controller = new AbortController();
  await run({ executable: "git", args: [], signal: controller.signal });
  controller.abort();
  await vi.runAllTimersAsync();
  expect(sendCancelProcessRequest).not.toHaveBeenCalled();
});
