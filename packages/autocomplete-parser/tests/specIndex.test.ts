import { expect, it } from "vitest";
import { parseSpecIndex } from "../src/loadHelpers";
it.each([
  null,
  [],
  {},
  { completions: [1], diffVersionedCompletions: [] },
  { completions: [], diffVersionedCompletions: null },
])("rejects a malformed spec index: %j", (value) => {
  expect(() => parseSpecIndex(value)).toThrow();
});
it("accepts only the two validated string arrays", () => {
  expect(
    parseSpecIndex({ completions: ["git"], diffVersionedCompletions: ["node"], extra: true }),
  ).toEqual({ completions: ["git"], diffVersionedCompletions: ["node"] });
});
