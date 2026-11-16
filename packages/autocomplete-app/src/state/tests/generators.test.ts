import { services } from "../../generators/tests/services";
import { afterEach, expect, it, vi } from "vitest";
import { createGeneratorRunner } from "../generators";
import type { NamedSetState, AutocompleteState } from "../types";
import type { GeneratorState } from "../../generators/helpers";
import { getCustomSuggestions } from "../../generators/customSuggestionsGenerator";
vi.mock("../../generators/customSuggestionsGenerator", () => ({ getCustomSuggestions: vi.fn() }));
afterEach(() => {
  vi.useRealTimers();
  vi.clearAllMocks();
});
const makeState = (): GeneratorState => ({
  generator: {},
  context: {
    currentWorkingDirectory: "/",
    currentProcess: "zsh",
    environmentVariables: {},
    sshPrefix: "",
    annotations: [],
    tokenArray: [],
    searchTerm: "",
  },
  loading: true,
  result: [],
});
it("cancels only replaced generations and suppresses their late results", async () => {
  const first = makeState();
  const second = makeState();
  let resolve!: (value: Fig.Suggestion[]) => void;
  vi.mocked(getCustomSuggestions)
    .mockImplementationOnce(
      () =>
        new Promise((done) => {
          resolve = done;
        }),
    )
    .mockImplementationOnce(() => new Promise(() => {}));
  const write = vi.fn<NamedSetState<AutocompleteState>>();
  const runner = createGeneratorRunner(write, () => services);
  runner.runGenerator({ kind: "runGenerator", generatorState: first });
  runner.runGenerator({ kind: "runGenerator", generatorState: second });
  const firstSignal = vi.mocked(getCustomSuggestions).mock.calls[0][2]!;
  const secondSignal = vi.mocked(getCustomSuggestions).mock.calls[1][2]!;
  runner.reconcile([second]);
  expect(firstSignal.aborted).toBe(true);
  expect(secondSignal.aborted).toBe(false);
  resolve([{ name: "stale" }]);
  await Promise.resolve();
  await Promise.resolve();
  expect(write).not.toHaveBeenCalled();
  runner.reconcile([]);
  expect(secondSignal.aborted).toBe(true);
});
it("never starts a superseded debounce", async () => {
  vi.useFakeTimers();
  const runner = createGeneratorRunner(vi.fn(), () => services);
  runner.runGenerator({ kind: "runGenerator", generatorState: makeState(), debounceMs: 200 });
  runner.reconcile([]);
  await vi.advanceTimersByTimeAsync(200);
  expect(getCustomSuggestions).not.toHaveBeenCalled();
});
