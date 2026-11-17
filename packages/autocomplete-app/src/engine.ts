import type { CompletionEvent } from "./state/model";
import type { TraceContext } from "./telemetry";
import type { createParseCoordinator } from "./parser/coordinator";
import { shellContextSelector } from "./state/generator-plan";
import type { AutocompleteState } from "./state/types";
import type { StoreApi } from "zustand/vanilla";
const completionInputChanged = (state: AutocompleteState, previous?: AutocompleteState) => {
  const context = shellContextSelector(state);
  const old = previous && shellContextSelector(previous);
  return (
    !previous ||
    state.command !== previous.command ||
    state.settings !== previous.settings ||
    state.figState.shellContext?.inputRevision !== previous.figState.shellContext?.inputRevision ||
    context.currentWorkingDirectory !== old?.currentWorkingDirectory ||
    context.currentProcess !== old?.currentProcess ||
    context.environmentVariables !== old?.environmentVariables ||
    state.figState.shellContext?.sessionId !== previous.figState.shellContext?.sessionId
  );
};

type Ports = {
  coordinator: ReturnType<typeof createParseCoordinator>;
  now: () => number;
  defer(callback: () => void): void;
  dispatch: (event: CompletionEvent) => void;
  measure?: (stage: string, milliseconds: number, context?: TraceContext) => void;
};
/** One owner of parsing. Render/mount timing never schedules computation. */
export function startCompletionEngine(store: StoreApi<AutocompleteState>, ports: Ports) {
  const coordinator = ports.coordinator;
  const now = ports.now;
  let cancel: (() => void) | undefined;
  let stopped = false;
  let invalidated = false;
  const run = (state: AutocompleteState, previous?: AutocompleteState) => {
    if (stopped || (!invalidated && !completionInputChanged(state, previous))) return;
    invalidated = false;
    const context = shellContextSelector(state);
    last = state;
    cancel?.();
    const start = now();
    cancel = coordinator.run(state.command, context, {
      loading: (parsing) => ports.dispatch({ kind: "parsing", value: parsing }),
      result: (result, backspaced, large) => {
        ports.measure?.("parse", now() - start, state.figState.shellContext);
        ports.dispatch({
          kind: "parsed",
          result,
          backspaced,
          large,
          input: state.figState.shellContext,
        });
      },
      error: (error) => ports.dispatch({ kind: "failed", error: String(error) }),
    });
  };
  let scheduled = false;
  const schedule = () => {
    if (scheduled || stopped) return;
    scheduled = true;
    ports.defer(() => {
      scheduled = false;
      if (!stopped) run(store.getState(), last);
    });
  };
  let last: AutocompleteState | undefined;
  const unsubscribe = store.subscribe((state, previous) => {
    if (completionInputChanged(state, previous)) {
      invalidated = true;
      cancel?.();
      schedule();
    }
  });
  // Remember the dispatched input, not the states caused by its own loading/results.
  schedule();
  return () => {
    stopped = true;
    unsubscribe();
    cancel?.();
  };
}
