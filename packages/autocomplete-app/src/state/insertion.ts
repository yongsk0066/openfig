import logger from "loglevel";
import type { StoreApi } from "zustand";
import type { Shell } from "@fig/api-bindings";
import type { Suggestion } from "@fig/shared/internal";
import { makeArray, longestCommonPrefix, ensureTrailingSlash } from "@fig/shared/utils";
import { SETTINGS } from "@fig/shared/settings";
import { type NamedSetState, type AutocompleteState, Visibility } from "./types";
import { isMatchingType, getNameMatch, getQueryTermForSuggestion } from "../suggestions/helpers";
import { InsertPrefixError } from "./errors";

const sendTextToTerminal = (
  state: AutocompleteState,
  item: Suggestion,
  text: string,
  isFullCompletion: boolean,
  rootCommand: string,
  insert: typeof Shell.insert,
) => {
  const {
    parserResult: { searchTerm },
    figState: { buffer },
  } = state;
  const queryTerm = getQueryTermForSuggestion(item, searchTerm);
  // See if we need to move the cursor
  let valToInsert = text;
  const isPath = item.type === "file" || item.type === "folder";
  const cursorIdx = isPath
    ? item.separatorToAdd
      ? valToInsert.lastIndexOf("{cursor}")
      : -1
    : valToInsert.indexOf("{cursor}");
  let moveCursor = "";
  let cursorLeft = 0;
  if (cursorIdx > -1) {
    valToInsert =
      valToInsert.slice(0, cursorIdx) + valToInsert.slice(cursorIdx + "{cursor}".length);
    cursorLeft = [...valToInsert.slice(cursorIdx).replace(/\n$/, "")].length;
    moveCursor = "\x1b[D".repeat(valToInsert.length - cursorIdx);
  }

  // Get the number of characters the user has already input so we can delete them
  let numCharsToDelete = 0;

  if ((item.type !== "auto-execute" && item.type !== "special") || !isFullCompletion) {
    // This assumes any search term that uses a space was done so by escaping
    // the space with a backslash.
    // A better solution is to map the search term to the actual user input string...
    numCharsToDelete = queryTerm.replace(/\s/, "\\ ").length;
  }

  if (item.type === "shortcut" && searchTerm.startsWith("?")) {
    numCharsToDelete += 1;
  }

  if (rootCommand === "?") {
    numCharsToDelete += 2;
  }

  // Don't delete characters if we don't have to
  if (valToInsert.startsWith(queryTerm) && numCharsToDelete >= queryTerm.length) {
    numCharsToDelete -= queryTerm.length;
    valToInsert = valToInsert.slice(queryTerm.length);
  }

  const finalStringToInsert = "\b".repeat(numCharsToDelete) + valToInsert + moveCursor;

  logger.info(`inserting: ${finalStringToInsert}`);

  const revision = state.figState.shellContext?.inputRevision;
  const structured = revision !== undefined;
  const immediate = valToInsert.endsWith("\n");
  const insertion = immediate ? valToInsert.slice(0, -1) : valToInsert;
  const deletedText = buffer.slice(
    Math.max(0, state.figState.cursorLocation - numCharsToDelete),
    state.figState.cursorLocation,
  );
  const result = insert(
    structured ? insertion : finalStringToInsert,
    structured
      ? {
          insertionBuffer: buffer,
          inputRevision: revision,
          expectedCursor: BigInt(state.figState.cursorLocation),
          deletion: BigInt([...deletedText].length),
          offset: BigInt(-cursorLeft),
          immediate,
        }
      : { insertionBuffer: buffer },
    state.figState.shellContext?.sessionId,
  );
  return result;
};

const insertString = (
  state: AutocompleteState,
  item: Suggestion,
  text: string,
  isFullCompletion: boolean,
  insert: typeof Shell.insert,
  rejected: (state: AutocompleteState) => void,
  recordUsage: (command: string, suggestion: string) => void,
) => {
  const { command, updateVisibilityPostInsert, parserResult } = state;
  const { commandIndex } = parserResult;
  const rootCommand = command?.tokens[commandIndex]?.text || "";

  // Mark a pending insertion before its E2 notification can arrive. Parser transitions use
  // this marker to distinguish our edit from a paste; usage history waits for the actual ACK.
  updateVisibilityPostInsert(item, isFullCompletion);
  void sendTextToTerminal(state, item, text, isFullCompletion, rootCommand, insert).then(
    () => {
      if (isFullCompletion) {
        const suggestionName = makeArray(item.name)[0] || "";
        recordUsage(rootCommand, suggestionName);
      }

      logger.info("Shell acknowledged insertion");
    },
    (error: unknown) => {
      logger.warn("Shell completion was not confirmed", error);
      rejected(state);
    },
  );
};

const escapeInsertion = (str: string, isFolder: boolean) => {
  // Keep ordinary space escaping, but quote shell syntax and non-space separators.
  // In particular a newline in a structured directory entry is part of one filename.
  if (!/[\\?*'"#|<>()[\]!&;$`{}~^=\t\r\n]/.test(str)) {
    return str.replace(/ /g, "\\ ");
  }

  if (isFolder) {
    return `'${str.slice(0, -1).replace(/'/g, "'\"'\"'")}'/`;
  }
  return `'${str.replace(/'/g, "'\"'\"'")}'`;
};

const getFullInsertion = (
  item: Suggestion,
  searchTerm: string,
  fuzzySearch: boolean,
  preferVerboseSuggestions: boolean,
): string => {
  const queryTerm = getQueryTermForSuggestion(item, searchTerm);

  const isFolder = item.type === "folder";
  const isFileOrFolder = item.type === "file" || isFolder;

  if (item.insertValue && !isFileOrFolder) {
    return item.insertValue;
  }

  const match = getNameMatch(item, queryTerm, fuzzySearch, preferVerboseSuggestions);
  if (!match) return escapeInsertion("", isFolder);
  return escapeInsertion(
    item.separatorToAdd ? `${match}${item.separatorToAdd}{cursor}` : match,
    isFolder,
  );
};

const makeInsertTextForItem =
  (
    get: StoreApi<AutocompleteState>["getState"],
    insert: typeof Shell.insert,
    rejected: (state: AutocompleteState) => void,
    recordUsage: (command: string, suggestion: string) => void,
  ) =>
  (item: Suggestion, execute = false) => {
    const state = get();
    // The displayed candidate may still belong to the previous parse while input is in flight.
    if (state.parsing) return;
    const {
      parserResult: { searchTerm },
      fuzzySearchEnabled,
      settings,
    } = state;
    let text = getFullInsertion(
      item,
      searchTerm,
      fuzzySearchEnabled,
      settings[SETTINGS.PREFER_VERBOSE_SUGGESTIONS] as boolean,
    );

    if (execute && item.type !== "auto-execute") {
      text += "\n";
    }
    // Replace multiple newlines at the end of string with just one.
    text = text.replace(/\n+$/g, "\n");

    if (
      !text.endsWith("\n") &&
      item.shouldAddSpace &&
      (settings[SETTINGS.INSERT_SPACE_AUTOMATICALLY] ?? true)
    ) {
      text += " ";
    }

    return insertString(state, item, text, true, insert, rejected, recordUsage);
  };

const makeInsertCommonPrefix =
  (
    get: StoreApi<AutocompleteState>["getState"],
    insert: typeof Shell.insert,
    rejected: (state: AutocompleteState) => void,
    recordUsage: (command: string, suggestion: string) => void,
  ) =>
  () => {
    const state = get();
    // The displayed candidate may still belong to the previous parse while input is in flight.
    if (state.parsing) return;
    const {
      suggestions,
      selectedIndex,
      parserResult: { searchTerm },
      fuzzySearchEnabled,
      settings,
      insertTextForItem,
    } = state;
    const selectedItem = suggestions[selectedIndex];

    const queryTerm = getQueryTermForSuggestion(selectedItem, searchTerm);
    if (suggestions.length === 1) {
      return insertTextForItem(selectedItem);
    }

    let { type: itemType } = selectedItem;

    if (itemType === "auto-execute") {
      itemType = selectedItem.originalType;
    }

    if (itemType && ["special", "auto-execute"].includes(itemType)) {
      throw new InsertPrefixError(`Cannot insert common prefix for type ${itemType}`);
    }

    const typeFilterItem = {
      ...selectedItem,
      type: itemType,
      name:
        selectedItem.type === "auto-execute" && selectedItem.originalType === "folder"
          ? makeArray(selectedItem.name ?? []).map(ensureTrailingSlash)
          : selectedItem.name,
    };

    const sameTypeSuggestionNames = suggestions
      .filter((suggestion) => isMatchingType(suggestion, typeFilterItem))
      .map((suggestion) => (makeArray(suggestion.name || [])[0] ?? "").toLowerCase())
      .filter((name) => name.startsWith(queryTerm.toLowerCase()));

    if (sameTypeSuggestionNames.length === 1) {
      if (selectedItem.type === "auto-execute") {
        throw new InsertPrefixError(`Cannot auto-execute using common prefix`);
      }
      return insertTextForItem(selectedItem);
    }

    const uncasedSharedPrefix = longestCommonPrefix(sameTypeSuggestionNames);
    const sharedPrefix = makeArray(selectedItem.name || [])[0].slice(0, uncasedSharedPrefix.length);
    if (!sharedPrefix || sharedPrefix === queryTerm) {
      throw new InsertPrefixError("No remaining prefix to insert");
    }
    const text = sharedPrefix.replace(/\s/g, "\\ ");
    if (
      text ===
      getFullInsertion(
        selectedItem,
        searchTerm,
        fuzzySearchEnabled,
        settings[SETTINGS.PREFER_VERBOSE_SUGGESTIONS] as boolean,
      )
    ) {
      if (selectedItem.type === "auto-execute") {
        throw new InsertPrefixError(`Cannot auto-execute using common prefix`);
      }
      return insertTextForItem(selectedItem);
    }
    return insertString(state, selectedItem, text, false, insert, rejected, recordUsage);
  };

export const createInsertionState = (
  setNamed: NamedSetState<AutocompleteState>,
  get: StoreApi<AutocompleteState>["getState"],
  insert: typeof Shell.insert,
  recordUsage: (command: string, suggestion: string) => void = () => {},
) => {
  const rejected = (before: AutocompleteState) =>
    setNamed("insertionRejected", (state) =>
      state.figState === before.figState
        ? { visibleState: Visibility.VISIBLE, justInserted: false, lastInsertedSuggestion: null }
        : {},
    );
  return {
    insertCommonPrefix: makeInsertCommonPrefix(get, insert, rejected, recordUsage),
    insertTextForItem: makeInsertTextForItem(get, insert, rejected, recordUsage),
    updateVisibilityPostInsert: (suggestion: Suggestion, isFullCompletion: boolean) => {
      if (isFullCompletion) {
        setNamed("updateVisibilityPostInsert", {
          visibleState: Visibility.HIDDEN_BY_INSERTION,
          lastInsertedSuggestion: suggestion,
          justInserted: true,
        });
      } else {
        setNamed("updateVisibilityPostInsert", { justInserted: true });
      }
    },
  };
};
