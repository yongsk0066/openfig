import { useEffect } from "react";
import { Keybindings } from "@fig/api-bindings";
import { subscribeWithRetry } from "../subscription";
export { initialFigState, type FigState } from "../state/fig-state";
export function useFigKeypress(callback: Parameters<typeof Keybindings.pressed>[0]) {
  useEffect(() => subscribeWithRetry(() => Keybindings.pressed(callback)), [callback]);
}
