import { useStore } from "zustand";
import { autocompleteStore } from "../fig/runtime";
import type { AutocompleteState } from "./types";
export const useAutocompleteStore = Object.assign(
  <T>(selector: (state: AutocompleteState) => T): T => useStore(autocompleteStore, selector),
  autocompleteStore,
);
