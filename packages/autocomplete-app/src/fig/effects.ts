import logger from "loglevel";
import { Keybindings } from "@fig/api-bindings";
import { ACTIONS } from "../actions";
import type { SetShellGlobalsEffect, InterceptKeystrokesEffect } from "../state/effects";
/** Runs every effect except `runGenerator`, which needs the store (see `createGeneratorRunner`). */
export const applyEffect = (effect: SetShellGlobalsEffect | InterceptKeystrokesEffect) => {
  switch (effect.kind) {
    case "setShellGlobals":
      Object.assign(window, effect.globals);
      break;
    case "setInterceptKeystrokes":
      logger.info("Setting intercept keystrokes", effect);
      void Promise.resolve(
        Keybindings.setInterceptKeystrokes(
          ACTIONS,
          effect.intercept,
          effect.globalIntercept,
          effect.sessionId,
        ),
      ).catch((error) => logger.warn("Unable to update shell key interception", error));
      break;
    default:
      break;
  }
};
