import { afterEach, expect, it, vi } from "vitest";

afterEach(() => {
  localStorage.clear();
  vi.unstubAllGlobals();
});
it("keeps late commits and equal revisions from different sessions separate", async () => {
  vi.resetModules();
  vi.stubGlobal("figRuntime", { epoch: "7" });
  vi.stubGlobal(
    "requestAnimationFrame",
    vi.fn(() => 1),
  );
  localStorage.setItem("fig.trace", "1");
  const { beginInput, recordCommit, timingSnapshot } = await import("./telemetry");
  const a = { sessionId: "a", inputRevision: 1n };
  const b = { sessionId: "b", inputRevision: 1n };
  vi.stubGlobal("figDeliveryId", "11");
  beginInput(a);
  vi.stubGlobal("figDeliveryId", "12");
  beginInput(b);
  recordCommit(a, true);
  recordCommit(a, true);
  recordCommit(b, false);
  expect(timingSnapshot().filter((sample) => sample.stage !== "input-received")).toMatchObject([
    {
      stage: "input-to-first-dom-commit",
      sessionId: "a",
      revision: "1",
      viewEpoch: "7",
      deliveryId: "11",
    },
    { stage: "input-to-settled-dom-commit", sessionId: "a", deliveryId: "11" },
    { stage: "input-to-first-dom-commit", sessionId: "b", deliveryId: "12" },
  ]);
});
it("excludes old/hidden lists and batches payload-free metrics", async () => {
  vi.useFakeTimers();
  vi.resetModules();
  vi.stubGlobal("figRuntime", { epoch: "8" });
  vi.stubGlobal("figLatencyTrace", true);
  const postMessage = vi.fn();
  vi.stubGlobal("ipc", { postMessage });
  vi.stubGlobal(
    "requestAnimationFrame",
    vi.fn(() => 1),
  );
  const { beginInput, recordCommit, timingSnapshot } = await import("./telemetry");
  const context = { sessionId: "session", inputRevision: 2n };
  try {
    beginInput(context);
    recordCommit(context, false, { current: false, visible: true, suggestionCount: 10 });
    recordCommit(context, true, { current: true, visible: false, suggestionCount: 10 });
    expect(timingSnapshot().some((s) => s.stage === "input-to-current-suggestions-dom")).toBe(
      false,
    );
    recordCommit(context, true, { current: true, visible: true, suggestionCount: 10 });
    expect(
      timingSnapshot().filter((s) => s.stage === "input-to-current-suggestions-dom"),
    ).toHaveLength(1);
    expect(postMessage).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1000);
    expect(postMessage).toHaveBeenCalledOnce();
    const payload = JSON.parse(postMessage.mock.calls[0][0].slice("fig-latency:".length));
    expect(
      payload.every((s: Record<string, unknown>) => !("buffer" in s) && !("current" in s)),
    ).toBe(true);
  } finally {
    vi.useRealTimers();
  }
});
it("records a visible frame even if a hidden settled frame already ran", async () => {
  vi.resetModules();
  localStorage.setItem("fig.trace", "1");
  const frames: FrameRequestCallback[] = [];
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
    frames.push(callback);
    return frames.length;
  });
  const { beginInput, recordCommit, timingSnapshot } = await import("./telemetry");
  const input = { sessionId: "s", inputRevision: 3n };
  beginInput(input);
  recordCommit(input, true, { current: true, visible: false, suggestionCount: 2 });
  frames.shift()!(0);
  recordCommit(input, true, { current: true, visible: true, suggestionCount: 2 });
  frames.shift()!(0);
  expect(
    timingSnapshot().filter((sample) => sample.stage === "current-suggestions-next-frame"),
  ).toHaveLength(1);
});
