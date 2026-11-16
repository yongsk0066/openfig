import { expect, it } from "vitest";
import { createPrefixIndex } from "./prefix-index";
it("matches a linear scan, preserving duplicate and source order across Unicode and replacement", () => {
  const search = createPrefixIndex<{ text: string }>();
  const entries = ["git z", "한글", "git a", "git z", "😀 x", "gi", "git", ""].map((text) => ({
    text,
  }));
  for (const prefix of ["", "gi", "git ", "한", "😀", "missing"])
    expect(search(entries, prefix)).toEqual(
      entries.filter((entry) => entry.text.startsWith(prefix)),
    );
  expect(search(entries, "git ")).toBe(search(entries, "git "));
  const replaced = [...entries, { text: "git new" }];
  expect(search(replaced, "git ")).toHaveLength(4);
});
