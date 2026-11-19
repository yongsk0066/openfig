import { create } from "zustand";
import { Settings as ApiSettings, State as ApiState, Install, Platform } from "@fig/api-bindings";
import type { PlatformInfo } from "@/types/preferences";

type KV = Record<string, unknown>;

export type SetupKey = "accessibility" | "dotfiles" | "ghosttyInputMethod" | "loginItem";
export type SetupCheck = { status?: Install.SetupStatus; error?: string; busy?: boolean };

export interface Data {
  error: string | undefined;
  settings: KV | undefined;
  state: KV | undefined;
  platformInfo: PlatformInfo | undefined;

  setup: Partial<Record<SetupKey, SetupCheck>>;
}

export interface Actions {
  setSetting: (key: string, value: unknown) => Promise<void>;
  setState: (key: string, value: unknown) => Promise<void>;
  refreshLocalState: () => Promise<void>;
  checkSetup: (key: SetupKey) => Promise<void>;
  configure: (key: SetupKey) => Promise<void>;

  isLoading: () => boolean;
}

export type State = Data & Actions;

export type Store = ReturnType<typeof createStore>;

export type DashboardPorts = {
  settings: Pick<typeof ApiSettings, "current" | "set" | "didChange">;
  state: Pick<typeof ApiState, "current" | "set" | "didChange">;
  install: Pick<typeof Install, "getStatus" | "install" | "installStatus">;
  platform: Pick<typeof Platform, "getPlatformInfo">;
};
export const createStore = (
  ports: DashboardPorts = {
    settings: ApiSettings,
    state: ApiState,
    install: Install,
    platform: Platform,
  },
) => {
  let generation = 0;
  let stop: (() => void) | undefined;
  const versions = new Map<string, number>();
  const confirmed = new Map<string, unknown>();
  const pending = new Map<string, Promise<void>>();
  const revisions = { settings: 0, state: 0 };
  const save = async (kind: "settings" | "state", key: string, value: unknown) => {
    const scope = generation;
    const id = `${kind}:${key}`;
    const version = (versions.get(id) ?? 0) + 1;
    versions.set(id, version);
    revisions[kind]++;
    if (!pending.has(id)) confirmed.set(id, store.getState()[kind]?.[key]);
    store.setState((s) => ({ [kind]: { ...s[kind], [key]: value }, error: undefined }));
    // Preserve server write order per key while keeping the UI optimistic.
    const previous = pending.get(id) ?? Promise.resolve();
    const write = previous.then(async () => {
      if (scope !== generation) return;
      try {
        await ports[kind].set(key, value);
        if (scope === generation) confirmed.set(id, value);
      } catch (error) {
        if (scope === generation && versions.get(id) === version)
          store.setState((s) => ({
            [kind]: { ...s[kind], [key]: confirmed.get(id) },
            error: String(error),
          }));
      }
    });
    pending.set(id, write);
    await write;
    if (pending.get(id) === write) {
      pending.delete(id);
      confirmed.delete(id);
    }
  };
  const refresh = async <T>(read: () => Promise<T>, apply: (value: T) => Partial<Data>) => {
    const scope = generation;
    try {
      const value = await read();
      if (scope === generation) store.setState(apply(value));
    } catch (error) {
      if (scope === generation) store.setState({ error: String(error) });
    }
  };
  const probes = new Map<SetupKey, number>();
  const patch = (key: SetupKey, change: SetupCheck) =>
    store.setState((s) => ({ setup: { ...s.setup, [key]: { ...s.setup[key], ...change } } }));
  const checkSetup = async (key: SetupKey) => {
    const scope = generation;
    const version = (probes.get(key) ?? 0) + 1;
    probes.set(key, version);
    const current = () => scope === generation && probes.get(key) === version;
    try {
      const status = await ports.install.getStatus(key);
      if (current()) patch(key, { status, error: undefined });
    } catch (error) {
      if (current()) patch(key, { status: undefined, error: String(error) });
    }
  };
  const configure = async (key: SetupKey) => {
    if (store.getState().setup[key]?.busy) return;
    const scope = generation;
    patch(key, { busy: true, error: undefined });
    try {
      await ports.install.install(key);
      if (scope === generation) await checkSetup(key);
    } catch (error) {
      if (scope === generation) {
        await checkSetup(key);
        if (scope === generation) patch(key, { error: String(error) });
      }
    } finally {
      if (scope === generation) patch(key, { busy: false });
    }
  };
  const store = create<State>()((_set, get) => ({
    error: undefined,
    settings: undefined,
    state: undefined,
    platformInfo: undefined,
    setup: {},
    setSetting: (key, value) => save("settings", key, value),
    setState: (key, value) => save("state", key, value),
    checkSetup,
    configure,
    refreshLocalState: () => refresh(ports.state.current, (state) => ({ state })),
    isLoading: () => {
      const s = get();
      return s.state === undefined || s.settings === undefined || s.platformInfo === undefined;
    },
  }));
  const start = () => {
    stop?.();
    const scope = ++generation;
    const active = () => scope === generation;
    const cleanups: (() => void)[] = [];
    const fail = (error: unknown) => {
      if (active()) store.setState({ error: String(error) });
    };
    const update = (value: Partial<Data>) => {
      if (active()) store.setState(value);
    };
    store.setState({ error: undefined });
    const subscribe = (request: Promise<{ unsubscribe(): void }> | undefined) => {
      void request?.then((subscription) => {
        if (active()) cleanups.push(() => subscription.unsubscribe());
        else subscription.unsubscribe();
      }, fail);
    };
    for (const kind of ["settings", "state"] as const) {
      const revision = revisions[kind];
      void ports[kind].current().then((value) => {
        if (revision === revisions[kind]) update({ [kind]: value });
      }, fail);
      subscribe(
        ports[kind].didChange.subscribe((notification) => {
          if (active())
            try {
              const value: unknown = JSON.parse(notification.jsonBlob ?? "{}");
              if (!value || typeof value !== "object" || Array.isArray(value))
                throw new Error(`Invalid ${kind} snapshot`);
              const snapshot = { ...value } as KV;
              revisions[kind]++;
              // A notification can describe an earlier write while a newer write is pending.
              for (const id of pending.keys())
                if (id.startsWith(`${kind}:`)) {
                  const key = id.slice(kind.length + 1);
                  snapshot[key] = store.getState()[kind]?.[key];
                }
              update({ [kind]: snapshot });
            } catch (error) {
              fail(error);
            }
          return { unsubscribe: !active() };
        }),
      );
    }
    void ports.platform.getPlatformInfo().then((platformInfo) => update({ platformInfo }), fail);
    store.setState({ setup: {} });
    void checkSetup("accessibility");
    void checkSetup("dotfiles");
    subscribe(
      ports.install.installStatus.onLoginItemChange(() => {
        if (active() && store.getState().setup.loginItem) void checkSetup("loginItem");
      }),
    );
    subscribe(
      ports.install.installStatus.subscribe("accessibility", (enabled) => {
        if (active()) {
          probes.set("accessibility", (probes.get("accessibility") ?? 0) + 1);
          patch("accessibility", { status: enabled ? "ready" : "missing", error: undefined });
        }
        return { unsubscribe: !active() };
      }),
    );
    const dispose = () => {
      if (!active()) return;
      generation++;
      cleanups.forEach((cleanup) => cleanup());
      stop = undefined;
    };
    stop = dispose;
    return dispose;
  };
  return Object.assign(store, { start, dispose: () => stop?.() });
};
