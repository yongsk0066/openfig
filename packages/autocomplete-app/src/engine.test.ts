import { createCompletionModel, type ModelPorts } from "./state/model";
const createAutocompleteStore = (ports: Partial<ModelPorts> = {}) =>
  createCompletionModel({
    effect: vi.fn(),
    generators: () => ({ reconcile: vi.fn(), runGenerator: vi.fn() }),
    insertion: vi.fn(),
    recordUsage: vi.fn(),
    history: () => ({}),
    recency: () => ({}),
    now: () => 0,
    measure: vi.fn(),
    report: vi.fn(),
    ...ports,
  });
// @vitest-environment node
import { expect, it, vi } from "vitest";
import { createStore } from "zustand/vanilla";
import type { createParseCoordinator } from "./parser/coordinator";
import { startCompletionEngine } from "./engine";
import { initialState } from "./state/transitions";
import type { AutocompleteState } from "./state/types";
it("schedules from inputs without React, coalesces synchronous updates and disposes", async () => {
  const run = vi.fn<ReturnType<typeof createParseCoordinator>["run"]>(() => vi.fn());
  const store = createStore<AutocompleteState>(() => ({ ...initialState }) as AutocompleteState);
  const stop = startCompletionEngine(store, { coordinator: { run }, dispatch: vi.fn() });
  store.setState({ figState: { ...store.getState().figState, cwd: "/a" } });
  store.setState({ figState: { ...store.getState().figState, cwd: "/b" } });
  await Promise.resolve();
  expect(run).toHaveBeenCalledTimes(1);
  expect(run.mock.calls[0]?.[1]).toMatchObject({ currentWorkingDirectory: "/b" });
  store.setState({ parsing: true });
  await Promise.resolve();
  expect(run).toHaveBeenCalledTimes(1);
  stop();
  store.setState({ figState: { ...store.getState().figState, cwd: "/c" } });
  await Promise.resolve();
  expect(run).toHaveBeenCalledTimes(1);
});

it("runs the real store without window and keeps platform effects behind ports", async () => {
  const effect = vi.fn();
  const store = createAutocompleteStore({
    effect,
    generators: () => ({ reconcile: vi.fn(), runGenerator: vi.fn() }),
  });
  const run = vi.fn<ReturnType<typeof createParseCoordinator>["run"]>(() => vi.fn());
  const stop = startCompletionEngine(store, { coordinator: { run }, dispatch: vi.fn() });
  store
    .getState()
    .setFigState((state) => ({ ...state, buffer: "git ", cursorLocation: 4, cwd: "/repo" }));
  await Promise.resolve();
  expect(run).toHaveBeenCalledTimes(1);
  expect(store.getState().figState.buffer).toBe("git ");
  expect(effect).toHaveBeenCalled();
  stop();
});

it("a rejected shell edit clears only its own pending UI state", async () => {
  const store = createAutocompleteStore({
    effect: vi.fn(),
    insertion: vi.fn().mockRejectedValue(new Error("stale input")),
    generators: () => ({ reconcile: vi.fn(), runGenerator: vi.fn() }),
  });
  store.getState().insertTextForItem({ name: "thing" });
  expect(store.getState().justInserted).toBe(true);
  await Promise.resolve();
  expect(store.getState().justInserted).toBe(false);
});

it("restarts a cancelled parse when coalesced context returns to the dispatched value", async () => {
  const cancel = vi.fn();
  const run = vi.fn<ReturnType<typeof createParseCoordinator>["run"]>(() => cancel);
  const store = createStore<AutocompleteState>(() => ({ ...initialState }) as AutocompleteState);
  const stop = startCompletionEngine(store, { coordinator: { run }, dispatch: vi.fn() });
  await Promise.resolve();
  const original = store.getState().figState;
  store.setState({ figState: { ...original, cwd: "/temporary" } });
  store.setState({ figState: original });
  await Promise.resolve();
  expect(cancel).toHaveBeenCalled();
  expect(run).toHaveBeenCalledTimes(2);
  stop();
});
