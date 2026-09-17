import { useCallback } from "react";
import logger from "loglevel";
import { Keybindings, Shell } from "@fig/api-bindings";
import { SETTINGS, getSetting } from "@fig/api-bindings/wrappers";
import { AutocompleteAction } from "../actions";
import { useShallow } from "zustand/shallow";
import { useAutocompleteStore } from "../state";
import { Visibility } from "../state/types";

enum KeyCode {
  TAB = 48,
}

export enum Direction {
  INCREASE = "increase",
  DECREASE = "decrease",
}

export const useAutocompleteKeypressCallback = (
  toggleDescriptionPopout: () => void,
  shake: () => void,
  changeSize: (direction: Direction) => void,
): Parameters<typeof Keybindings.pressed>[0] => {
  // Only the five reactive values this hook reads; everything else in the store
  // (parser result, generator states, history mode, …) no longer rebuilds the
  // keypress callback on every keystroke.
  const { suggestions, visibleState, selectedIndex, scrollWrapAround } = useAutocompleteStore(
    useShallow((state) => ({
      suggestions: state.suggestions,
      visibleState: state.visibleState,
      selectedIndex: state.selectedIndex,
      scrollWrapAround: state.settings[SETTINGS.SCROLL_WRAP_AROUND],
    })),
  );

  const setHistoryModeEnabled = useAutocompleteStore((state) => state.setHistoryModeEnabled);
  const setUserFuzzySearchEnabled = useAutocompleteStore(
    (state) => state.setUserFuzzySearchEnabled,
  );
  const insertTextForItem = useAutocompleteStore((state) => state.insertTextForItem);
  const insertCommonPrefix = useAutocompleteStore((state) => state.insertCommonPrefix);
  const scrollToIndex = useAutocompleteStore((state) => state.scroll);
  const setVisibleState = useAutocompleteStore((state) => state.setVisibleState);
  const setFigState = useAutocompleteStore((state) => state.setFigState);

  const selectedItem = suggestions[selectedIndex];

  const navigate = useCallback(
    (delta: number, hideOrShowHistoryOnNegative = false) => {
      let newIndex = selectedIndex + delta;
      let newVisibleState = visibleState;
      if (scrollWrapAround) {
        newIndex = (newIndex + suggestions.length) % suggestions.length;
      } else if (newIndex === -1 && hideOrShowHistoryOnNegative) {
        if (getSetting<boolean>(SETTINGS.NAVIGATE_TO_HISTORY)) {
          setHistoryModeEnabled((enabled) => !enabled);
          return;
        }
        newVisibleState = Visibility.HIDDEN_UNTIL_KEYPRESS;
      }
      scrollToIndex(newIndex, newVisibleState);
    },
    [selectedIndex, visibleState, suggestions.length, scrollWrapAround, setHistoryModeEnabled],
  );

  return useCallback(
    (notification) => {
      const { action, keypress, context: shellContext } = notification;
      logger.debug("Handling keypress", { notification, keypress, action });

      // Read before the write so the EXECUTE branch below keeps using the
      // session id from *before* this keypress, as it always has.
      const previousShellContext = useAutocompleteStore.getState().figState.shellContext;
      setFigState((state) => ({ ...state, shellContext }));

      if (
        getSetting<boolean>(SETTINGS.ONLY_SHOW_ON_TAB) &&
        keypress?.appleKeyCode === KeyCode.TAB &&
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
            shake();
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
          Shell.insert("\n", undefined, previousShellContext?.sessionId);
          break;
        case AutocompleteAction.HIDE_AUTOCOMPLETE:
          setVisibleState(Visibility.HIDDEN_UNTIL_SHOWN);
          break;
        case AutocompleteAction.SHOW_AUTOCOMPLETE:
          setVisibleState(Visibility.VISIBLE);
          break;
        case AutocompleteAction.TOGGLE_AUTOCOMPLETE:
          setVisibleState(
            visibleState === Visibility.VISIBLE
              ? Visibility.HIDDEN_UNTIL_SHOWN
              : Visibility.VISIBLE,
          );
          break;
        case AutocompleteAction.TOGGLE_HISTORY_MODE:
          setHistoryModeEnabled((enabled) => !enabled);
          break;
        case AutocompleteAction.TOGGLE_DESCRIPTION:
          if (!getSetting<boolean>(SETTINGS.ALWAYS_SHOW_DESCRIPTION)) {
            toggleDescriptionPopout();
          }
          break;
        case AutocompleteAction.TOGGLE_FUZZY_SEARCH:
          setUserFuzzySearchEnabled((enabled) => !enabled);
          break;
        case AutocompleteAction.INCREASE_SIZE:
          changeSize(Direction.INCREASE);
          break;
        case AutocompleteAction.DECREASE_SIZE:
          changeSize(Direction.DECREASE);
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
          const suggestionIndex = parseInt(action, 10) - 1;
          if (
            suggestionIndex <= suggestions.length - 1 &&
            suggestionIndex <= 0 &&
            suggestionIndex <= 9
          ) {
            scrollToIndex(suggestionIndex, Visibility.VISIBLE);
          }
          break;
        }
        default:
          logger.error("Unrecognized action");
          break;
      }
      return undefined;
    },
    [
      navigate,
      insertCommonPrefix,
      insertTextForItem,
      selectedItem,
      toggleDescriptionPopout,
      setHistoryModeEnabled,
      visibleState,
      setVisibleState,
      changeSize,
      setFigState,
    ],
  );
};
