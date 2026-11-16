import { EditBufferNotifications, Event, Settings, Shell } from "@fig/api-bindings";
import { updateSettings } from "@fig/api-bindings/wrappers";
import logger from "loglevel";
import { beginInput, recordTiming } from "../telemetry";
import { createCompletionRuntime } from "../runtime";
import { createDesktopCompletion } from "./services";
import { updateSelectSuggestionKeybindings } from "../actions";
const runtime = createCompletionRuntime({
  completion: createDesktopCompletion(),
  beginInput,
  measure: recordTiming,
  input: EditBufferNotifications.subscribe,
  process: Shell.processDidChange.subscribe,
  history: Shell.historyUpdated.subscribe,
  settings: Settings.didChange.subscribe,
  currentSettings: Settings.current,
  clearCache: (callback) => Event.subscribe("clear-cache", callback),
  ready: async (signal) => {
    const epoch = await window.figRuntime?.ready;
    signal.throwIfAborted();
    window.ipc?.postMessage?.(`fig-runtime:ready:${epoch}`);
  },
  settingsApplied: (value) => {
    updateSettings(value);
    updateSelectSuggestionKeybindings(value);
  },
  report: (error) => logger.warn("Completion runtime", error),
});
export const autocompleteStore = runtime.store;
export const startAutocompleteRuntime = () => runtime.start();

export const disposeAutocompleteRuntime = () => runtime.dispose();
