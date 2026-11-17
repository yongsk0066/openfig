import { createErrorInstance } from "./errors";
import type { Timing } from "./timing";
export const TimeoutError = createErrorInstance("TimeoutError");

/** Stops observing on timeout/abort; callers own cancellation of the underlying operation. */
export async function withTimeout<T>(
  time: number,
  promise: Promise<T>,
  signal?: AbortSignal,
): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  let onAbort: (() => void) | undefined;
  try {
    return await Promise.race([
      promise,
      new Promise<never>((_, reject) => {
        onAbort = () => reject(signal?.reason);
        if (signal?.aborted) {
          onAbort();
          return;
        }
        signal?.addEventListener("abort", onAbort, { once: true });
        timer = setTimeout(() => reject(new TimeoutError("Function timed out")), time);
      }),
    ]);
  } finally {
    clearTimeout(timer);
    if (onAbort) signal?.removeEventListener("abort", onAbort);
  }
}

export function sleep(milliseconds: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    signal?.throwIfAborted();
    const finish = () => {
      signal?.removeEventListener("abort", abort);
      resolve();
    };
    const timer = setTimeout(finish, milliseconds);
    const abort = () => {
      clearTimeout(timer);
      signal?.removeEventListener("abort", abort);
      reject(signal?.reason);
    };
    signal?.addEventListener("abort", abort, { once: true });
  });
}
export const runtimeTiming: Timing = {
  now: () => performance.now(),
  defer: (callback) => queueMicrotask(callback),
  schedule: (callback, milliseconds) => {
    const timer = setTimeout(callback, milliseconds);
    return () => clearTimeout(timer);
  },
  delay: sleep,
  timeout: withTimeout,
};
