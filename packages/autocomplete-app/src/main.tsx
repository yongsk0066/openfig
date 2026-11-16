import { installGlobalAPI } from "@fig/api-bindings";
import React from "react";
import logger from "loglevel";
import ReactDOM from "react-dom/client";
import App from "./App";
import { recordTiming, timingSnapshot } from "./telemetry";
import { startAutocompleteRuntime, disposeAutocompleteRuntime } from "./fig/runtime";
import ErrorBoundary from "./components/ErrorBoundary";

const reportError = (event: ErrorEvent) => {
  logger.error(
    event.error ?? new Error(`${event.filename}:${event.lineno}:${event.colno}: ${event.message}`),
  );
};
window.addEventListener("error", reportError);
if (import.meta.hot)
  import.meta.hot.dispose(() => window.removeEventListener("error", reportError));

window.globalCWD = "";
window.globalSSHString = "";
window.globalTerminalSessionId = "";
window.logger = logger;

logger.setDefaultLevel("warn");

installGlobalAPI();
Object.defineProperty(window, "figTimingSnapshot", { value: timingSnapshot, configurable: true });
startAutocompleteRuntime();
if (import.meta.hot) import.meta.hot.dispose(disposeAutocompleteRuntime);
ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ErrorBoundary>
      <React.Profiler
        id="autocomplete"
        onRender={(_id, _phase, duration) => recordTiming("react-render", duration)}
      >
        <App />
      </React.Profiler>
    </ErrorBoundary>
  </React.StrictMode>,
);
