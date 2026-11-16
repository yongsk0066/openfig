import { fs } from "@fig/api-bindings";
import { isInDevMode } from "@fig/api-bindings/wrappers";

// Logging functions
const DEFAULT_CONSOLE = {
  log: console.log,
  warn: console.warn,
  error: console.error,
};

export const LOG_DIR = typeof window === "undefined" ? undefined : window.fig?.constants?.logsDir;

const formatLogValue = (value: unknown): string => {
  if (typeof value === "string") return value;
  if (value instanceof Error) return value.stack ?? value.message;
  try {
    return JSON.stringify(value) ?? String(value);
  } catch {
    return String(value);
  }
};

const formatLog = (content: unknown[]): string => content.map(formatLogValue).join(" ");

const NEW_LOG_FN = (...content: unknown[]) => {
  void fs.append(`${LOG_DIR}/logs/specs.log`, `\n${formatLog(content)}`).then(
    () => DEFAULT_CONSOLE.warn("SPEC LOG:", formatLog(content)),
    (error) => DEFAULT_CONSOLE.warn("SPEC LOG:", formatLog(content), "Write failed", error),
  );
};

export function runPipingConsoleMethods<T>(fn: () => T) {
  try {
    pipeConsoleMethods();
    return fn();
  } finally {
    restoreConsoleMethods();
  }
}

export function pipeConsoleMethods() {
  if (isInDevMode()) {
    console.log = NEW_LOG_FN;
    console.warn = NEW_LOG_FN;
    console.error = NEW_LOG_FN;
  }
}

export function restoreConsoleMethods() {
  if (isInDevMode()) {
    console.log = DEFAULT_CONSOLE.log;
    console.warn = DEFAULT_CONSOLE.warn;
    console.error = DEFAULT_CONSOLE.error;
  }
}
