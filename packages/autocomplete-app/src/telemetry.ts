export type TraceContext = { sessionId?: string; inputRevision?: bigint };
type Sample = {
  stage: string;
  milliseconds: number;
  unixMs: number;
  clockDifferenceMs: number;
  sessionId?: string;
  revision?: string;
  viewEpoch?: string;
  deliveryId?: string;
  visible?: boolean;
  suggestionCount?: number;
};
type Display = { visible: boolean; suggestionCount: number; current: boolean };
const samples: Sample[] = [];
const pending: Sample[] = [];
let flushTimer: ReturnType<typeof setTimeout> | undefined;
const flush = () => {
  if (flushTimer !== undefined) clearTimeout(flushTimer);
  flushTimer = undefined;
  if (pending.length) window.ipc?.postMessage?.(`fig-latency:${JSON.stringify(pending.splice(0))}`);
};
const inputs = new Map<
  string,
  {
    started: number;
    deliveryId?: string;
    first: boolean;
    settled: boolean;
    current: boolean;
    frameScheduled: boolean;
    currentFrameRecorded: boolean;
    settledFrameRecorded: boolean;
  }
>();
function enabled() {
  try {
    return (
      typeof window !== "undefined" &&
      (window.figLatencyTrace || localStorage.getItem("fig.trace") === "1")
    );
  } catch {
    return false;
  }
}
function identity(context?: TraceContext) {
  return {
    sessionId: context?.sessionId,
    revision: context?.inputRevision?.toString(),
    viewEpoch: typeof window === "undefined" ? undefined : window.figRuntime?.epoch,
  };
}
function key(context?: TraceContext) {
  return JSON.stringify(identity(context));
}
/** Identifiers and timings only. Diagnostic IPC is opt-in and batched off the input path. */
export function recordTiming(
  stage: string,
  milliseconds: number,
  context?: TraceContext,
  display?: Pick<Display, "visible" | "suggestionCount">,
) {
  if (!enabled()) return;
  const unixMs = Date.now();
  const sample: Sample = {
    stage,
    milliseconds,
    unixMs,
    clockDifferenceMs: unixMs - (performance.timeOrigin + performance.now()),
    ...identity(context),
    deliveryId: inputs.get(key(context))?.deliveryId,
    ...display,
  };
  if (samples.length >= 512) samples.shift();
  samples.push(sample);
  if (window.figLatencyTrace) {
    if (pending.length === 512) flush();
    pending.push(sample);
    flushTimer ??= setTimeout(flush, 1000);
  }
}
export const timingSnapshot = () => samples.slice();
export function beginInput(context?: TraceContext) {
  if (!enabled()) return;
  const id = key(context);
  if (inputs.has(id)) return;
  if (inputs.size >= 128) inputs.delete(inputs.keys().next().value!);
  inputs.set(id, {
    started: performance.now(),
    deliveryId: window.figDeliveryId,
    first: false,
    settled: false,
    current: false,
    frameScheduled: false,
    currentFrameRecorded: false,
    settledFrameRecorded: false,
  });
  recordTiming("input-received", 0, context);
}
export function recordCommit(
  context: TraceContext | undefined,
  settled: boolean,
  display?: Display,
) {
  if (!enabled()) return;
  const input = inputs.get(key(context));
  if (!input) return;
  // Do not serialize the parser-state flag; only aggregate display metadata crosses IPC.
  const metadata = display && {
    visible: display.visible,
    suggestionCount: display.suggestionCount,
  };
  if (!input.first) {
    input.first = true;
    recordTiming("input-to-first-dom-commit", performance.now() - input.started, context, metadata);
  }
  if (settled && !input.settled) {
    input.settled = true;
    recordTiming(
      "input-to-settled-dom-commit",
      performance.now() - input.started,
      context,
      metadata,
    );
  }
  const current = Boolean(display?.current && display.visible && display.suggestionCount > 0);
  if (current && !input.current) {
    input.current = true;
    recordTiming(
      "input-to-current-suggestions-dom",
      performance.now() - input.started,
      context,
      metadata,
    );
  }
  const currentFrame = current && !input.currentFrameRecorded;
  const settledFrame = settled && !input.settledFrameRecorded;
  if ((currentFrame || settledFrame) && !input.frameScheduled) {
    input.frameScheduled = true;
    const frame = requestAnimationFrame(() => {
      input.frameScheduled = false;
      // A requestAnimationFrame callback is a rendering opportunity, not pixel presentation.
      if (currentFrame) {
        input.currentFrameRecorded = true;
        recordTiming(
          "current-suggestions-next-frame",
          performance.now() - input.started,
          context,
          metadata,
        );
      }
      if (settledFrame) {
        input.settledFrameRecorded = true;
        recordTiming(
          "settled-next-animation-frame",
          performance.now() - input.started,
          context,
          metadata,
        );
      }
    });
    return () => {
      cancelAnimationFrame(frame);
      input.frameScheduled = false;
    };
  }
  return undefined;
}
declare global {
  interface Window {
    figDeliveryId?: string;
    figLatencyTrace?: boolean;
  }
}
