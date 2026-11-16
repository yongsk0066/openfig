import { describe, expect, it, vi } from "vitest";
import { createStore, type DashboardPorts } from "./store";
const deferred = <T>() => {
  let resolve!: (value: T) => void, reject!: (error: Error) => void;
  const promise = new Promise<T>((a, b) => {
    resolve = a;
    reject = b;
  });
  return { promise, resolve, reject };
};
function fixture() {
  const unsubscribe = vi.fn();
  const ports: DashboardPorts = {
    settings: {
      current: vi.fn(async () => ({ theme: "old" })),
      set: vi.fn(async () => {}),
      didChange: { subscribe: vi.fn(async () => ({ unsubscribe })) },
    },
    state: {
      current: vi.fn(async () => ({})),
      set: vi.fn(async () => {}),
      didChange: { subscribe: vi.fn(async () => ({ unsubscribe })) },
    },
    install: {
      isInstalled: vi.fn(async () => true),
      installStatus: { subscribe: vi.fn(async () => ({ unsubscribe })) },
    },
    platform: { getPlatformInfo: vi.fn(async () => ({}) as never) },
  };
  return { ports, unsubscribe, store: createStore(ports) };
}
describe("dashboard store ownership", () => {
  it("constructs without subscriptions and cleans up a late registration", async () => {
    const { ports, store, unsubscribe } = fixture();
    const pending = deferred<{ unsubscribe(): void }>();
    vi.mocked(ports.settings.didChange.subscribe).mockReturnValue(pending.promise);
    expect(ports.settings.current).not.toHaveBeenCalled();
    const stop = store.start();
    stop();
    pending.resolve({ unsubscribe });
    await vi.waitFor(() => expect(unsubscribe).toHaveBeenCalledTimes(3));
    expect(store.getState().settings).toBeUndefined();
  });
  it("keeps notifications newer than the startup snapshot", async () => {
    const { ports, store } = fixture();
    const pending = deferred<Record<string, unknown>>();
    vi.mocked(ports.settings.current).mockReturnValue(pending.promise);
    store.start();
    vi.mocked(ports.settings.didChange.subscribe).mock.calls[0][0]({
      jsonBlob: '{"theme":"new"}',
    } as never);
    pending.resolve({ theme: "old" });
    await Promise.resolve();
    expect(store.getState().settings).toEqual({ theme: "new" });
    store.dispose();
  });
  it("orders concurrent writes and rolls back to the last confirmed value", async () => {
    const { ports, store } = fixture();
    store.setState({ settings: { theme: "original" } });
    const first = deferred<void>(),
      second = deferred<void>();
    vi.mocked(ports.settings.set)
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(second.promise);
    const a = store.getState().setSetting("theme", "a"),
      b = store.getState().setSetting("theme", "b");
    await Promise.resolve();
    expect(ports.settings.set).toHaveBeenCalledTimes(1);
    expect(store.getState().settings?.theme).toBe("b");
    first.resolve();
    await a;
    await Promise.resolve();
    second.reject(new Error("save failed"));
    await b;
    expect(store.getState().settings?.theme).toBe("a");
    expect(store.getState().error).toContain("save failed");
  });
  it("ignores a save rejection after unmount", async () => {
    const { ports, store } = fixture();
    store.start();
    await vi.waitFor(() => expect(store.getState().isLoading()).toBe(false));
    const pending = deferred<void>();
    vi.mocked(ports.settings.set).mockReturnValue(pending.promise);
    const save = store.getState().setSetting("theme", "new");
    await Promise.resolve();
    store.dispose();
    const state = store.getState();
    pending.reject(new Error("late"));
    await save;
    expect(store.getState()).toBe(state);
  });
});
it("does not undo a newer accessibility notification with an old probe", async () => {
  const { ports, store } = fixture();
  const pending = deferred<boolean>();
  vi.mocked(ports.install.isInstalled).mockReturnValueOnce(pending.promise);
  store.start();
  vi.mocked(ports.install.installStatus.subscribe).mock.calls[0][1](true);
  pending.resolve(false);
  await Promise.resolve();
  expect(store.getState().accessibilityIsInstalled).toBe(true);
  store.dispose();
});
it("dispose closes the current subscriptions after an explicit retry", async () => {
  const { store, unsubscribe } = fixture();
  store.start();
  await Promise.resolve();
  store.start();
  await Promise.resolve();
  store.dispose();
  expect(unsubscribe).toHaveBeenCalledTimes(6);
});
