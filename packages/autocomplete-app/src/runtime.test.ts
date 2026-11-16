import { afterEach, describe, expect, it, vi } from "vitest";
import { SETTINGS } from "@fig/shared/settings";
import { createCompletionRuntime, type RuntimePorts } from "./runtime";
import { createCompletionModel } from "./state/model";
import { initialParserState } from "@fig/autocomplete-parser";
const deferred = <T>() => {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
};
function fixture() {
  const effect = vi.fn(),
    generators = { reconcile: vi.fn(), runGenerator: vi.fn() };
  const store = createCompletionModel({
    effect,
    generators: () => generators,
    insertion: vi.fn(),
    recordUsage: vi.fn(),
    history: () => ({}) as never,
    recency: () => ({}),
    now: () => 0,
    measure: vi.fn(),
    report: vi.fn(),
  });
  const subscriptions = Array.from({ length: 5 }, () => ({ unsubscribe: vi.fn() }));
  const ports: RuntimePorts = {
    completion: {
      store,
      parse: vi.fn(async () => initialParserState),
      settings: vi.fn((settings) => store.dispatch({ kind: "settings", update: settings })),
      loadHistory: vi.fn(async () => {}),
      refreshHistory: vi.fn(async () => {}),
      clear: vi.fn(),
      dispose: vi.fn(() => store.dispose()),
    },
    input: vi.fn(async () => subscriptions[0]),
    process: vi.fn(async () => subscriptions[1]),
    settings: vi.fn(async () => subscriptions[2]),
    clearCache: vi.fn(async () => subscriptions[3]),
    history: vi.fn(async () => subscriptions[4]),
    currentSettings: vi.fn(async () => ({})),
    ready: vi.fn(async () => {}),
    settingsApplied: vi.fn(),
    report: vi.fn(),
    beginInput: vi.fn(),
    measure: vi.fn(),
  };
  return { ports, subscriptions, runtime: createCompletionRuntime(ports), effect, generators };
}
afterEach(() => vi.useRealTimers());
describe("completion lifecycle", () => {
  it("does not let a stopped startup publish settings, history or ready", async () => {
    const { ports, runtime, subscriptions } = fixture();
    const first = deferred<Record<string, unknown>>();
    vi.mocked(ports.currentSettings).mockReturnValueOnce(first.promise);
    const stop = runtime.start();
    await vi.waitFor(() => expect(ports.currentSettings).toHaveBeenCalledOnce());
    stop();
    runtime.start();
    await vi.waitFor(() => expect(ports.ready).toHaveBeenCalledOnce());
    first.resolve({ old: true });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(ports.completion.settings).toHaveBeenCalledTimes(1);
    expect(ports.completion.settings).toHaveBeenCalledWith({});
    expect(ports.completion.loadHistory).toHaveBeenCalledOnce();
    runtime.dispose();
    expect(subscriptions.every((s) => s.unsubscribe.mock.calls.length === 2)).toBe(true);
  });
  it("unsubscribes registrations that complete after disposal", async () => {
    const { ports, runtime } = fixture();
    const pending = deferred<{ unsubscribe(): void }>();
    const unsubscribe = vi.fn();
    vi.mocked(ports.input).mockReturnValue(pending.promise);
    runtime.start();
    runtime.dispose();
    pending.resolve({ unsubscribe });
    await vi.waitFor(() => expect(unsubscribe).toHaveBeenCalledOnce());
    expect(ports.ready).not.toHaveBeenCalled();
    expect(() => runtime.start()).toThrow("disposed");
  });
  it("preserves a newer settings notification over an initial snapshot", async () => {
    const { ports, runtime } = fixture();
    const pending = deferred<Record<string, unknown>>();
    vi.mocked(ports.currentSettings).mockReturnValue(pending.promise);
    runtime.start();
    await vi.waitFor(() => expect(ports.currentSettings).toHaveBeenCalledOnce());
    vi.mocked(ports.settings).mock.calls[0][0]({
      jsonBlob: JSON.stringify({ [SETTINGS.DISABLE_HISTORY_LOADING]: true }),
    } as never);
    pending.resolve({ old: true });
    await vi.waitFor(() => expect(ports.ready).toHaveBeenCalledOnce());
    expect(ports.completion.settings).toHaveBeenCalledExactlyOnceWith({
      [SETTINGS.DISABLE_HISTORY_LOADING]: true,
    });
    expect(ports.completion.loadHistory).not.toHaveBeenCalled();
    runtime.dispose();
  });
  it("isolates model state and cancels generators on disposal", () => {
    const a = fixture(),
      b = fixture();
    a.runtime.store.dispatch({ kind: "settings", update: { [SETTINGS.FUZZY_SEARCH]: true } });
    expect(b.runtime.store.getState().settings).toEqual({});
    a.runtime.dispose();
    const state = a.runtime.store.getState();
    a.runtime.store.dispatch({ kind: "settings", update: { [SETTINGS.FUZZY_SEARCH]: false } });
    expect(a.runtime.store.getState()).toBe(state);
    expect(a.generators.reconcile).toHaveBeenLastCalledWith([]);
    b.runtime.dispose();
  });
});
it("stopping clears cancelled generator plans so a restart can schedule them again", () => {
  const { runtime } = fixture();
  const stop = runtime.start();
  runtime.store.setState({ generatorStates: [{ loading: true } as never] });
  stop();
  expect(runtime.store.getState().generatorStates).toEqual([]);
  expect(runtime.store.getState().parsing).toBe(false);
  runtime.dispose();
});
it("coalesces history notifications and ignores them after shutdown", async () => {
  const { runtime, ports } = fixture();
  runtime.start();
  await vi.waitFor(() => expect(ports.ready).toHaveBeenCalledOnce());
  const pending = deferred<void>();
  vi.mocked(ports.completion.refreshHistory).mockReturnValueOnce(pending.promise);
  const notify = vi.mocked(ports.history).mock.calls[0][0];
  for (let i = 0; i < 50; i++) notify({} as never);
  expect(ports.completion.refreshHistory).toHaveBeenCalledOnce();
  pending.resolve();
  await vi.waitFor(() => expect(ports.completion.refreshHistory).toHaveBeenCalledTimes(2));
  runtime.dispose();
  notify({} as never);
  expect(ports.completion.refreshHistory).toHaveBeenCalledTimes(2);
});
