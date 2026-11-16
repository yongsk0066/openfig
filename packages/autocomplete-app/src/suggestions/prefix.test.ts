import { expect, it } from "vitest";
import type { Suggestion } from "@fig/shared/internal";
import { ensureTrailingSlash, makeArray } from "@fig/shared/utils";
import { getCommonSuggestionPrefix, isMatchingType } from "./helpers";
const reference = (selectedIndex: number, suggestions: Suggestion[]) => {
  const item = suggestions[selectedIndex];
  const type = item.type === "auto-execute" ? item.originalType : item.type;
  if (type === "special" || type === "auto-execute") return null;
  const filter = {
    ...item,
    type,
    name:
      item.type === "auto-execute" && item.originalType === "folder"
        ? makeArray(item.name ?? []).map(ensureTrailingSlash)
        : item.name,
  };
  const names = suggestions
    .filter((s) => isMatchingType(s, filter))
    .map((s) => (makeArray(s.name || [])[0] ?? "").toLowerCase())
    .toSorted();
  if (names.length < 2) return null;
  let length = 0;
  while (length < names[0].length && names[0][length] === names.at(-1)?.[length]) length++;
  return names[0].slice(0, length);
};
it("memoized prefix matches the previous algorithm across types, aliases, case and parent-folder selection", () => {
  const suggestions: Suggestion[] = [
    { name: "../", type: "folder" },
    { name: "Project/", type: "folder" },
    { name: "project.txt", type: "file" },
    { name: ["print", "alias"], type: "subcommand" },
    { name: "preview", type: "subcommand" },
    { name: "Project", type: "auto-execute", originalType: "folder" },
    { name: "../", type: "auto-execute", originalType: "folder" },
    { name: "special", type: "special" },
    { name: "plain" },
    { name: "plan" },
    { name: "", type: "arg" },
  ];
  for (let round = 0; round < 3; round++)
    for (let i = suggestions.length - 1; i >= 0; i--)
      expect(getCommonSuggestionPrefix(i, suggestions)).toBe(reference(i, suggestions));
  expect(getCommonSuggestionPrefix(0, [])).toBeNull();
  expect(getCommonSuggestionPrefix(0, [{ name: "new" }, { name: "next" }])).toBe("ne");
});
