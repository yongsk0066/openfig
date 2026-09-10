import { StoreContext } from "@/context/zustand";
import { useCallback, useContext } from "react";
import { useStore } from "zustand";

/**
 * @param key The id of the setting
 * @returns An array with the current value of the setting and a callback to set the value
 */
export function useSetting(key: string) {
  const store = useContext(StoreContext);
  if (!store) throw new Error("Missing StoreContext.Provider in the tree");

  // zustand 5 re-renders whenever a selector returns a new reference, so the
  // setter must come from a stable store action rather than an inline closure.
  const value = useStore(store, (state) => state.settings![key]);
  const setSetting = useStore(store, (state) => state.setSetting);
  const set = useCallback((next: unknown) => setSetting(key, next), [setSetting, key]);
  return [value, set] as const;
}
