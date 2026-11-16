import type { Keybindings } from "@fig/api-bindings";
import type { StoreApi } from "zustand/vanilla";
import { SETTINGS, readSettings } from "@fig/shared/settings";
export enum AutocompleteAction {
  INSERT_SELECTED = "insertSelected",
  INSERT_COMMON_PREFIX = "insertCommonPrefix",
  INSERT_COMMON_PREFIX_OR_NAVIGATE_DOWN = "insertCommonPrefixOrNavigateDown",
  INSERT_COMMON_PREFIX_OR_INSERT_SELECTED = "insertCommonPrefixOrInsertSelected",
  INSERT_SELECTED_AND_EXECUTE = "insertSelectedAndExecute",
  EXECUTE = "execute",

  HIDE_AUTOCOMPLETE = "hideAutocomplete",
  SHOW_AUTOCOMPLETE = "showAutocomplete",
  TOGGLE_AUTOCOMPLETE = "toggleAutocomplete",

  NAVIGATE_UP = "navigateUp",
  NAVIGATE_DOWN = "navigateDown",

  HIDE_DESCRIPTION = "hideDescription",
  SHOW_DESCRIPTION = "showDescription",
  TOGGLE_DESCRIPTION = "toggleDescription",

  TOGGLE_HISTORY_MODE = "toggleHistoryMode",

  TOGGLE_FUZZY_SEARCH = "toggleFuzzySearch",

  INCREASE_SIZE = "increaseSize",
  DECREASE_SIZE = "decreaseSize",

  SELECT_SUGGESTION_1 = "selectSuggestion1",
  SELECT_SUGGESTION_2 = "selectSuggestion2",
  SELECT_SUGGESTION_3 = "selectSuggestion3",
  SELECT_SUGGESTION_4 = "selectSuggestion4",
  SELECT_SUGGESTION_5 = "selectSuggestion5",
  SELECT_SUGGESTION_6 = "selectSuggestion6",
  SELECT_SUGGESTION_7 = "selectSuggestion7",
  SELECT_SUGGESTION_8 = "selectSuggestion8",
  SELECT_SUGGESTION_9 = "selectSuggestion9",
  SELECT_SUGGESTION_10 = "selectSuggestion10",
}

import { Visibility, type AutocompleteState } from "./types";
export enum Direction {
  INCREASE = "increase",
  DECREASE = "decrease",
}
export type KeypressPorts = {
  execute(sessionId?: string): Promise<unknown>;
  toggleDescription(): void;
  shake(): void;
  changeSize(direction: Direction): void;
  report(error: unknown): void;
};
/** Interpret a key intent against the current committed model, independent of React renders. */
export function createKeypressHandler(
  store: StoreApi<AutocompleteState>,
  ports: KeypressPorts,
): Parameters<typeof Keybindings.pressed>[0] {
  return (notification) => {
    const state = store.getState();
    const {
      suggestions,
      visibleState,
      selectedIndex,
      setHistoryModeEnabled,
      setUserFuzzySearchEnabled,
      insertTextForItem,
      insertCommonPrefix,
      scroll: scrollToIndex,
      setVisibleState,
      setFigState,
    } = state;
    const read = readSettings(state.settings);
    const selectedItem = suggestions[selectedIndex];
    const navigate = (delta: number, hideOrShowHistoryOnNegative = false) => {
      let newIndex = selectedIndex + delta;
      let newVisibleState = visibleState;
      if (read<boolean>(SETTINGS.SCROLL_WRAP_AROUND) && suggestions.length > 0)
        newIndex = (newIndex + suggestions.length) % suggestions.length;
      else if (newIndex === -1 && hideOrShowHistoryOnNegative) {
        if (read<boolean>(SETTINGS.NAVIGATE_TO_HISTORY)) {
          setHistoryModeEnabled((enabled) => !enabled);
          return;
        }
        newVisibleState = Visibility.HIDDEN_UNTIL_KEYPRESS;
      }
      scrollToIndex(newIndex, newVisibleState);
    };
    const { action, keypress, context: shellContext } = notification;

    // Read before the write so the EXECUTE branch below keeps using the
    // session id from *before* this keypress, as it always has.
    const previousShellContext = store.getState().figState.shellContext;
    setFigState((previous) => ({ ...previous, shellContext }));

    if (
      read<boolean>(SETTINGS.ONLY_SHOW_ON_TAB) &&
      keypress?.appleKeyCode === 48 &&
      visibleState !== Visibility.VISIBLE
    ) {
      if (suggestions.length === 1) {
        insertTextForItem(suggestions[0]);
        return undefined;
      }
      setVisibleState(Visibility.VISIBLE);
      return undefined;
    }
    if (!selectedItem) {
      return undefined;
    }
    switch (action) {
      case AutocompleteAction.NAVIGATE_UP:
        navigate(-1, true);
        break;
      case AutocompleteAction.NAVIGATE_DOWN:
        navigate(1);
        break;
      case AutocompleteAction.INSERT_SELECTED:
        insertTextForItem(selectedItem);
        break;
      case AutocompleteAction.INSERT_COMMON_PREFIX:
        try {
          insertCommonPrefix();
        } catch (_err) {
          ports.shake();
        }
        break;
      case AutocompleteAction.INSERT_COMMON_PREFIX_OR_NAVIGATE_DOWN:
        try {
          insertCommonPrefix();
        } catch (_err) {
          navigate(1);
        }
        break;
      case AutocompleteAction.INSERT_COMMON_PREFIX_OR_INSERT_SELECTED:
        try {
          insertCommonPrefix();
        } catch (_err) {
          insertTextForItem(selectedItem);
        }
        break;
      case AutocompleteAction.INSERT_SELECTED_AND_EXECUTE:
        insertTextForItem(selectedItem, true);
        break;
      case AutocompleteAction.EXECUTE:
        void ports.execute(previousShellContext?.sessionId).catch(ports.report);
        break;
      case AutocompleteAction.HIDE_AUTOCOMPLETE:
        setVisibleState(Visibility.HIDDEN_UNTIL_SHOWN);
        break;
      case AutocompleteAction.SHOW_AUTOCOMPLETE:
        setVisibleState(Visibility.VISIBLE);
        break;
      case AutocompleteAction.TOGGLE_AUTOCOMPLETE:
        setVisibleState(
          visibleState === Visibility.VISIBLE ? Visibility.HIDDEN_UNTIL_SHOWN : Visibility.VISIBLE,
        );
        break;
      case AutocompleteAction.TOGGLE_HISTORY_MODE:
        setHistoryModeEnabled((enabled) => !enabled);
        break;
      case AutocompleteAction.TOGGLE_DESCRIPTION:
        if (!read<boolean>(SETTINGS.ALWAYS_SHOW_DESCRIPTION)) {
          ports.toggleDescription();
        }
        break;
      case AutocompleteAction.TOGGLE_FUZZY_SEARCH:
        setUserFuzzySearchEnabled((enabled) => !enabled);
        break;
      case AutocompleteAction.INCREASE_SIZE:
        ports.changeSize(Direction.INCREASE);
        break;
      case AutocompleteAction.DECREASE_SIZE:
        ports.changeSize(Direction.DECREASE);
        break;
      case AutocompleteAction.SELECT_SUGGESTION_1:
      case AutocompleteAction.SELECT_SUGGESTION_2:
      case AutocompleteAction.SELECT_SUGGESTION_3:
      case AutocompleteAction.SELECT_SUGGESTION_4:
      case AutocompleteAction.SELECT_SUGGESTION_5:
      case AutocompleteAction.SELECT_SUGGESTION_6:
      case AutocompleteAction.SELECT_SUGGESTION_7:
      case AutocompleteAction.SELECT_SUGGESTION_8:
      case AutocompleteAction.SELECT_SUGGESTION_9:
      case AutocompleteAction.SELECT_SUGGESTION_10: {
        const suggestionIndex = Number(action.slice("selectSuggestion".length)) - 1;
        if (
          suggestionIndex <= suggestions.length - 1 &&
          suggestionIndex >= 0 &&
          suggestionIndex <= 9
        ) {
          scrollToIndex(suggestionIndex, Visibility.VISIBLE);
        }
        break;
      }
      default:
        ports.report(new Error(`Unrecognized action: ${action}`));
        break;
    }
    return undefined;
  };
}
