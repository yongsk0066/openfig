// @vitest-environment node
import { expect, it, vi } from "vitest";
import { execFileSync } from "node:child_process";
import { createCompletionModel, type ModelPorts } from "../model";

it.each([
  "normal.txt",
  "{cursor}",
  "folder{cursor}/",
  "=ls",
  "a b",
  "a\tb",
  "line\nbreak",
  "trailing\n",
  "quote'file",
  "dollar$HOME",
  "semi;colon",
  "`name`",
  "{one,two}",
  "~name",
  "folder\nname/",
])("inserts %j as one literal shell path", (name) => {
  const insertion = vi.fn<ModelPorts["insertion"]>(async () => {});
  const store = createCompletionModel({
    effect: vi.fn(),
    generators: () => ({ reconcile: vi.fn(), runGenerator: vi.fn() }),
    insertion,
    recordUsage: vi.fn(),
    history: () => ({}),
    recency: () => ({}),
    now: () => 0,
    measure: vi.fn(),
    report: vi.fn(),
  });
  store.getState().insertTextForItem({ name, type: name.endsWith("/") ? "folder" : "file" });
  expect(insertion).toHaveBeenCalledOnce();
  const text = insertion.mock.calls[0][0];
  expect(text.endsWith("\n")).toBe(false);
  const output = execFileSync("/bin/zsh", ["-f", "-c", `set -- ${text}; printf '%s\\0' "$@"`], {
    env: { HOME: "/sentinel", PATH: "/usr/bin:/bin" },
  });
  expect(output.toString()).toBe(`${name}\0`);
  store.dispose();
});
