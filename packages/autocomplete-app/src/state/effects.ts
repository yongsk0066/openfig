import logger from "loglevel";
import { Keybindings } from "@fig/api-bindings";
import { ACTIONS } from "../actions";
import type { GeneratorState } from "../generators/helpers";
import { type AutocompleteState, Visibility } from "./types";

/**
 * Side effects the store's transitions ask for. Transitions return them instead of doing
 * them; they run after the state they belong to has been written.
 */
export type RunGeneratorEffect = {
  kind: "runGenerator";
  /** The (loading) state stored for this generator; its result is matched back by identity. */
  generatorState: GeneratorState;
  /** Wait this long first, and start only if `generatorState` is still the stored one. */
  debounceMs?: number;
};

/** Globals that specs' `executeShellCommand` and generators read. */
export type ShellGlobals = Partial<
  Pick<Window, "globalCWD" | "globalSSHString" | "globalTerminalSessionId">
>;

export type SetShellGlobalsEffect = { kind: "setShellGlobals"; globals: ShellGlobals };

export type InterceptKeystrokesEffect = {
  kind: "setInterceptKeystrokes";
  intercept: boolean;
  globalIntercept: boolean;
  sessionId: string | undefined;
};

export type Effect = RunGeneratorEffect | SetShellGlobalsEffect | InterceptKeystrokesEffect;

/** The desktop app routes the autocomplete keybindings here while the list is shown. */
export const interceptKeystrokesEffect = (state: AutocompleteState): InterceptKeystrokesEffect => ({
  kind: "setInterceptKeystrokes",
  intercept: state.visibleState === Visibility.VISIBLE && state.suggestions.length > 0,
  globalIntercept: state.suggestions.length > 0,
  sessionId: state.figState.shellContext?.sessionId,
});

/**
 * Effects owed by any store write, from the states before and after it. Same trigger as the
 * `useEffect(..., [interceptKeystrokes, suggestions.length])` in `App` this replaces.
 */
export const storeEffects = (
  before: AutocompleteState,
  after: AutocompleteState,
): InterceptKeystrokesEffect[] => {
  const was = interceptKeystrokesEffect(before);
  const is = interceptKeystrokesEffect(after);
  return was.intercept !== is.intercept || before.suggestions.length !== after.suggestions.length
    ? [is]
    : [];
};

/** Runs every effect except `runGenerator`, which needs the store (see `createGeneratorRunner`). */
export const applyEffect = (effect: SetShellGlobalsEffect | InterceptKeystrokesEffect) => {
  switch (effect.kind) {
    case "setShellGlobals":
      Object.assign(window, effect.globals);
      break;
    case "setInterceptKeystrokes":
      logger.info("Setting intercept keystrokes", effect);
      Keybindings.setInterceptKeystrokes(
        ACTIONS,
        effect.intercept,
        effect.globalIntercept,
        effect.sessionId,
      );
      break;
    default:
      break;
  }
};
