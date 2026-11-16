import { expect, it, vi } from "vitest";
import { readSettings } from "@fig/shared/settings";
import {
  createGeneratorCache,
  type GeneratorServices,
  type GeneratorState,
} from "../../generators/helpers";
import { createGeneratorRunner } from "../generators";
import { initialState } from "../transitions";
import type { AutocompleteState, NamedSetState } from "../types";
it("publishes cached and refreshed suggestions through the reconciled store, never after replacement", async () => {
  let finish!: (value: Fig.Suggestion[]) => void;
  const generator: Fig.Generator = {
    cache: { strategy: "stale-while-revalidate", ttl: -1 },
    custom: () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  };
  const context = {
    currentWorkingDirectory: "/repo",
    currentProcess: "zsh",
    environmentVariables: {},
    sshPrefix: "",
    tokenArray: [],
    annotations: [],
    searchTerm: "",
  };
  const cache = createGeneratorCache(readSettings({}));
  const services: GeneratorServices = {
    cached: cache.runCachedGenerator,
    execute: vi.fn(),
    invoke: (fn) => fn(),
    history: () => [],
    timeout: () => 5000,
  };
  await services.cached(generator, context, async () => [{ name: "old" }]);
  const planned: GeneratorState = { generator, context, loading: true, result: [] };
  let state = { ...initialState, generatorStates: [planned] } as AutocompleteState;
  const write: NamedSetState<AutocompleteState> = (_name, update) => {
    state = { ...state, ...(typeof update === "function" ? update(state) : update) };
    runner.reconcile(state.generatorStates);
  };
  const runner = createGeneratorRunner(write, () => services);
  runner.runGenerator({ kind: "runGenerator", generatorState: planned });
  await vi.waitFor(() => expect(state.generatorStates[0].result[0]?.name).toBe("old"));
  finish([{ name: "fresh" }]);
  await vi.waitFor(() => expect(state.generatorStates[0].result[0]?.name).toBe("fresh"));
  const replacement = { ...planned };
  state = { ...state, generatorStates: [replacement] };
  runner.reconcile([replacement]);
  runner.runGenerator({ kind: "runGenerator", generatorState: replacement });
  await vi.waitFor(() => expect(state.generatorStates[0].result[0]?.name).toBe("fresh"));
  runner.reconcile([]);
  finish([{ name: "cancelled" }]);
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(state.generatorStates[0].result[0]?.name).toBe("fresh");
});
