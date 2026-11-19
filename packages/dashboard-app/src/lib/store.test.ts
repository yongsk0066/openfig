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
      getStatus: vi.fn(async () => "ready" as const),
      install: vi.fn(async () => {}),
      installStatus: {
        onLoginItemChange: vi.fn(async () => ({ unsubscribe })),
        subscribe: vi.fn(async () => ({ unsubscribe })),
      },
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
    await vi.waitFor(() => expect(unsubscribe).toHaveBeenCalledTimes(4));
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
  const pending = deferred<"missing">();
  vi.mocked(ports.install.getStatus).mockReturnValueOnce(pending.promise);
  store.start();
  vi.mocked(ports.install.installStatus.subscribe).mock.calls[0][1](true);
  pending.resolve("missing");
  await Promise.resolve();
  expect(store.getState().setup.accessibility?.status).toBe("ready");
  store.dispose();
});
it("dispose closes the current subscriptions after an explicit retry", async () => {
  const { store, unsubscribe } = fixture();
  store.start();
  await Promise.resolve();
  store.start();
  await Promise.resolve();
  store.dispose();
  expect(unsubscribe).toHaveBeenCalledTimes(8);
});

it("keeps consent pending after a successful request and coalesces repeat clicks", async () => {
  const { ports, store } = fixture();
  const request = deferred<void>();
  vi.mocked(ports.install.install).mockReturnValueOnce(request.promise);
  vi.mocked(ports.install.getStatus)
    .mockResolvedValueOnce("approval")
    .mockResolvedValueOnce("selection")
    .mockResolvedValueOnce("ready");
  const first = store.getState().configure("ghosttyInputMethod");
  await store.getState().configure("ghosttyInputMethod");
  expect(ports.install.install).toHaveBeenCalledTimes(1);
  request.resolve();
  await first;
  expect(store.getState().setup.ghosttyInputMethod).toMatchObject({
    busy: false,
    status: "approval",
  });
  await store.getState().checkSetup("ghosttyInputMethod");
  expect(store.getState().setup.ghosttyInputMethod?.status).toBe("selection");
  await store.getState().configure("ghosttyInputMethod");
  expect(store.getState().setup.ghosttyInputMethod?.status).toBe("ready");
});

it("keeps setup failures on the affected component and allows a retry", async () => {
  const { ports, store } = fixture();
  vi.mocked(ports.install.install).mockRejectedValueOnce(
    new Error("CLI path belongs to another installation"),
  );
  vi.mocked(ports.install.getStatus).mockResolvedValueOnce("missing");
  await store.getState().configure("dotfiles");
  expect(store.getState().setup.dotfiles?.error).toContain("CLI path");
  expect(store.getState().error).toBeUndefined();
  await store.getState().configure("dotfiles");
  expect(store.getState().setup.dotfiles).toMatchObject({
    busy: false,
    status: "ready",
    error: undefined,
  });
});
