import type { Timing } from "@fig/shared/timing";
import type { EditBufferNotifications, Settings, Shell } from "@fig/api-bindings";
import { SETTINGS, type SettingsMap } from "@fig/shared/settings";
import { startCompletionEngine } from "./engine";
import { createParseCoordinator } from "./parser/coordinator";
import type { createCompletionModel } from "./state/model";
import { subscribeWithRetry as retrySubscription } from "./subscription";
import type { TraceContext } from "./telemetry";

type Subscription = { unsubscribe(): void };
export type RuntimePorts = {
  timing: Timing;
  completion: {
    store: ReturnType<typeof createCompletionModel>;
    parse: Parameters<typeof createParseCoordinator>[0];
    settings(value: SettingsMap): void;
    loadHistory(signal: AbortSignal): Promise<void>;
    refreshHistory(signal: AbortSignal): Promise<void>;
    clear(): void;
    dispose(): void;
  };
  input: typeof EditBufferNotifications.subscribe;
  process: typeof Shell.processDidChange.subscribe;
  history: typeof Shell.historyUpdated.subscribe;
  settings: typeof Settings.didChange.subscribe;
  currentSettings(): Promise<SettingsMap>;
  clearCache(callback: () => { unsubscribe: boolean }): Promise<Subscription> | undefined;
  ready(signal: AbortSignal): Promise<void>;
  settingsApplied(value: SettingsMap): void;
  report(error: unknown): void;
  beginInput(context?: TraceContext): void;
  measure(stage: string, milliseconds: number, context?: TraceContext): void;
};
/** Each start owns a cancellation scope. Old continuations can never join a later start. */
export function createCompletionRuntime(ports: RuntimePorts) {
  const subscribeWithRetry = (subscribe: Parameters<typeof retrySubscription>[0]) =>
    retrySubscription(subscribe, ports.timing.delay);
  let stop: (() => void) | undefined;
  let disposed = false;
  const { completion } = ports;
  const start = () => {
    if (disposed) throw new Error("Completion runtime disposed");
    if (stop) throw new Error("Completion runtime already started");
    const scope = new AbortController();
    const active = () => !scope.signal.aborted;
    const engine = startCompletionEngine(completion.store, {
      now: ports.timing.now,
      defer: ports.timing.defer,
      coordinator: createParseCoordinator(completion.parse),
      dispatch: completion.store.dispatch,
      measure: ports.measure,
    });
    const applySettings = (settings: SettingsMap) => {
      if (!active()) return;
      completion.settings(settings);
      ports.settingsApplied(settings);
    };
    completion.store.syncEffects();
    let settingsRevision = 0;
    let historyDirty = false;
    let refreshing = false;
    const refreshHistory = () => {
      if (!active() || completion.store.getState().settings[SETTINGS.DISABLE_HISTORY_LOADING])
        return;
      historyDirty = true;
      if (refreshing) return;
      refreshing = true;
      void (async () => {
        try {
          while (active() && historyDirty) {
            historyDirty = false;
            await completion.refreshHistory(scope.signal);
          }
        } catch (error) {
          if (active()) ports.report(error);
        } finally {
          refreshing = false;
        }
      })();
    };
    const unsubscribe = subscribeWithRetry(async () => {
      const revision = settingsRevision;
      const results = await Promise.allSettled([
        ports.input((notification) => {
          if (active()) {
            ports.beginInput(notification.context);
            completion.store.dispatch({
              kind: "input",
              update: (state) => ({
                ...state,
                buffer: notification.buffer ?? "",
                cursorLocation: notification.cursor ?? notification.buffer?.length ?? 0,
                cwd: notification.context?.currentWorkingDirectory ?? null,
                shellContext: notification.context,
              }),
            });
          }
          return { unsubscribe: !active() };
        }),
        ports.process(({ newProcess }) => {
          if (active())
            completion.store.dispatch({
              kind: "input",
              update: (state) => ({
                ...state,
                processUserIsIn: newProcess?.executable ?? null,
                cwd: newProcess?.directory ?? null,
              }),
            });
          return { unsubscribe: !active() };
        }),
        ports.settings(({ jsonBlob }) => {
          if (active()) {
            try {
              applySettings(JSON.parse(jsonBlob ?? "{}"));
              settingsRevision++;
            } catch (error) {
              ports.report(error);
            }
          }
          return { unsubscribe: !active() };
        }),
        ports.history(() => {
          refreshHistory();
          return { unsubscribe: !active() };
        }),
        ports.clearCache(() => {
          if (active()) completion.clear();
          return { unsubscribe: !active() };
        }),
      ]);
      const subscriptions = results.flatMap((result) =>
        result.status === "fulfilled" && result.value ? [result.value] : [],
      );
      const cleanup = () => subscriptions.forEach((subscription) => subscription.unsubscribe());
      if (!active()) {
        cleanup();
        return { unsubscribe: () => {} };
      }
      try {
        const failure = results.find((result) => result.status === "rejected");
        if (failure?.status === "rejected") throw failure.reason;
        const settings = await ports.currentSettings();
        if (!active()) {
          cleanup();
          return { unsubscribe: () => {} };
        }
        if (revision === settingsRevision) applySettings(settings);
        if (!completion.store.getState().settings[SETTINGS.DISABLE_HISTORY_LOADING])
          void completion.loadHistory(scope.signal).catch((error) => {
            if (active()) ports.report(error);
          });
        if (active()) await ports.ready(scope.signal);
      } catch (error) {
        cleanup();
        throw error;
      }
      return { unsubscribe: cleanup };
    });
    const dispose = () => {
      if (!active()) return;
      scope.abort();
      unsubscribe();
      engine();
      completion.store.cancelGenerators();
      stop = undefined;
    };
    stop = dispose;
    return dispose;
  };
  return {
    store: completion.store,
    start,
    dispose() {
      if (disposed) return;
      stop?.();
      disposed = true;
      completion.dispose();
    },
  };
}
