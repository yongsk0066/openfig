import { useMemo } from "react";
import logger from "loglevel";
import { Shell } from "@fig/api-bindings";
import { autocompleteStore } from "../fig/runtime";
import { createKeypressHandler, type Direction } from "../state/keypress";
export { Direction } from "../state/keypress";
export function useAutocompleteKeypressCallback(
  toggleDescriptionPopout: () => void,
  shake: () => void,
  changeSize: (direction: Direction) => void,
) {
  return useMemo(
    () =>
      createKeypressHandler(autocompleteStore, {
        execute: (sessionId) => Shell.insert("\n", undefined, sessionId),
        toggleDescription: toggleDescriptionPopout,
        shake,
        changeSize,
        report: (error) => logger.warn(error),
      }),
    [toggleDescriptionPopout, shake, changeSize],
  );
}
