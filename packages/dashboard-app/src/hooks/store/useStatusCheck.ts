import type { SetupKey } from "@/lib/store";
import { useStoreContext } from "./useStoreContext";

export function useSetup(key: SetupKey) {
  return useStoreContext((state) => state.setup[key]);
}
