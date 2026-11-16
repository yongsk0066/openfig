import type { SettingsMap } from "@fig/api-bindings/wrappers";
import * as app from "./fig.json";
import { create } from "@bufbuild/protobuf";
import { ActionAvailability, ActionSchema } from "@fig/proto/fig";

const SELECT_SUGGESTION_ACTION_PREFIX = "selectSuggestion";

export { AutocompleteAction } from "./state/keypress";

export const ACTIONS = app.contributes.actions.map((action) => {
  let availability: ActionAvailability | undefined;
  switch (action.availability) {
    case "ALWAYS":
      availability = ActionAvailability.ALWAYS;
      break;
    case "WHEN_FOCUSED":
      availability = ActionAvailability.WHEN_FOCUSED;
      break;
    case "WHEN_HIDDEN":
      availability = ActionAvailability.WHEN_HIDDEN;
      break;
    case "WHEN_VISIBLE":
      availability = ActionAvailability.WHEN_VISIBLE;
      break;
  }

  return create(ActionSchema, {
    ...action,
    availability,
  });
});

const defaultSelectSuggestionKeybindings = app.contributes.actions.reduce((p, v) => {
  if (v.identifier.startsWith("selectSuggestion")) {
    return Object.assign(p, { [v.identifier]: v.defaultBindings });
  }
  return p;
}, {});
let selectSuggestionKeybindings: Record<string, string[]> = {};

export const updateSelectSuggestionKeybindings = (settings: SettingsMap) => {
  selectSuggestionKeybindings = {};
  for (const [key, value] of Object.entries(settings)) {
    if (typeof value === "string" && value.startsWith(SELECT_SUGGESTION_ACTION_PREFIX)) {
      const keybinding = key.replace("autocomplete.keybindings.", "");
      if (!selectSuggestionKeybindings[value]) {
        selectSuggestionKeybindings[value] = [keybinding];
      } else {
        selectSuggestionKeybindings[value].push(keybinding);
      }
    }
  }

  selectSuggestionKeybindings = {
    ...defaultSelectSuggestionKeybindings,
    ...selectSuggestionKeybindings,
  };
};

export const getSelectSuggestionKeybinding = (
  suggestionIndex: number /* This is an index so it starts from 0 */,
) => selectSuggestionKeybindings[`selectSuggestion${suggestionIndex + 1}`];

export const getActionKeybindings = () => {};
