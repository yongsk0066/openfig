import { describe, expect, it, vi } from "vitest";
import { createStore } from "zustand/vanilla";
import { createKeypressHandler, AutocompleteAction } from "../keypress";
import { Visibility, type AutocompleteState } from "../types";
import { SETTINGS } from "@fig/shared/settings";
function fixture() {
  const store = createStore<AutocompleteState>(
    () =>
      ({
        settings: {},
        suggestions: Array.from({ length: 10 }, (_, i) => ({ name: String(i) })),
        selectedIndex: 0,
        visibleState: Visibility.VISIBLE,
        figState: {},
        setFigState: vi.fn(),
        scroll: vi.fn(),
        setHistoryModeEnabled: vi.fn(),
        insertTextForItem: vi.fn(),
      }) as unknown as AutocompleteState,
  );
  const handler = createKeypressHandler(store, {
    execute: vi.fn(async () => {}),
    toggleDescription: vi.fn(),
    shake: vi.fn(),
    changeSize: vi.fn(),
    report: vi.fn(),
  });
  return { store, send: (action: string) => handler({ action } as never) };
}
describe("keypress policy", () => {
  it.each([1, 2, 9, 10])("selects suggestion %i", (index) => {
    const { store, send } = fixture();
    send(`selectSuggestion${index}`);
    expect(store.getState().scroll).toHaveBeenCalledWith(index - 1, Visibility.VISIBLE);
  });
  it("reads current selection and settings without a React render", () => {
    const { store, send } = fixture();
    store.setState({ selectedIndex: 9, settings: { [SETTINGS.SCROLL_WRAP_AROUND]: true } });
    send(AutocompleteAction.NAVIGATE_DOWN);
    expect(store.getState().scroll).toHaveBeenCalledWith(0, Visibility.VISIBLE);
  });
});
