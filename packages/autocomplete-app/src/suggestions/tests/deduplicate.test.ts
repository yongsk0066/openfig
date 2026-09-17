import type { Suggestion } from "@fig/shared/internal";
import { fieldsAreEqual } from "@fig/shared/utils";
import { describe, expect, it } from "vitest";
import { deduplicateSuggestions } from "..";

const FIELDS: (keyof Suggestion)[] = ["name", "insertValue", "displayName", "args"];

/** The implementation before the Map-based one, kept as the oracle. */
const quadratic = (suggestions: Suggestion[], fields: (keyof Suggestion)[]) => {
  const deduplicated: Suggestion[] = [];
  for (const suggestion of suggestions) {
    if (deduplicated.filter((dd) => fieldsAreEqual(suggestion, dd, fields)).length === 0) {
      deduplicated.push(suggestion);
    }
  }
  return deduplicated;
};

const generator = { script: ["git", "branch"] } as Fig.Generator;

/**
 * 500 suggestions shaped like a merged spec + generator + history list: names as strings
 * and arrays, options with deep `args`, and every kind of near-duplicate — a structural
 * copy (new identity), the same name with another insert value / display name / args, a
 * string name vs a one-element array, and same-name rows that differ only in fields the
 * dedup ignores.
 */
const fixture = (): Suggestion[] => {
  const out: Suggestion[] = [];
  for (let i = 0; i < 125; i += 1) {
    const n = i % 90; // names repeat across the 125 groups
    const args = [{ name: `arg-${n % 7}`, generators: [generator], isOptional: n % 2 === 0 }];
    const option: Suggestion = {
      name: [`--flag-${n}`, `-${n}`],
      type: "option",
      args,
      priority: 50 + (i % 3),
    };
    out.push(
      option,
      // Structural duplicate with a new identity and a field the dedup ignores.
      { ...option, args: [{ ...args[0] }], description: `copy ${i}` },
      i % 4 === 0
        ? { name: `branch-${n}`, type: "arg" }
        : { name: [`branch-${n}`], type: "arg", insertValue: i % 4 === 1 ? `b-${n}` : undefined },
      i % 5 === 0
        ? { name: `sub-${n % 40}`, type: "subcommand", displayName: `Sub ${n % 3}` }
        : { name: `sub-${n % 40}`, type: "subcommand", icon: `icon-${i}` },
    );
  }
  return out;
};

describe("deduplicateSuggestions", () => {
  it("returns exactly what the quadratic version returned on 500 suggestions", () => {
    const suggestions = fixture();
    expect(suggestions).toHaveLength(500);
    const expected = quadratic(suggestions, FIELDS);
    const actual = deduplicateSuggestions(suggestions, FIELDS);
    // Same rows, same order, same object identities.
    expect(actual).toHaveLength(expected.length);
    actual.forEach((suggestion, index) => expect(suggestion).toBe(expected[index]));
    // The fixture really has duplicates of every kind.
    expect(expected.length).toBeLessThan(400);
    expect(expected.length).toBeGreaterThan(100);
  });

  it.each([
    [["name"]],
    [["name", "insertValue"]],
    [["displayName", "args"]],
    [["args"]],
  ] as (keyof Suggestion)[][][])("matches the quadratic version for fields %j", (fields) => {
    const suggestions = fixture();
    expect(deduplicateSuggestions(suggestions, fields)).toEqual(quadratic(suggestions, fields));
  });

  it("treats a missing field and an explicit undefined as equal, and string vs array as not", () => {
    const a: Suggestion = { name: "x" };
    const b: Suggestion = { name: "x", insertValue: undefined };
    const c: Suggestion = { name: ["x"] };
    expect(deduplicateSuggestions([a, b, c], FIELDS)).toEqual(quadratic([a, b, c], FIELDS));
    expect(deduplicateSuggestions([a, b, c], FIELDS)).toEqual([a, c]);
  });

  it("dedups 500 suggestions in at most 1 ms", () => {
    const suggestions = fixture();
    for (let i = 0; i < 20; i += 1) deduplicateSuggestions(suggestions, FIELDS);
    const times: number[] = [];
    for (let i = 0; i < 21; i += 1) {
      const start = performance.now();
      deduplicateSuggestions(suggestions, FIELDS);
      times.push(performance.now() - start);
    }
    times.sort((x, y) => x - y);
    expect(times[10]).toBeLessThanOrEqual(1);
  });
});
