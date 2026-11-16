import { expect, it, vi } from "vitest";
import { createParseCoordinator, bufferChange } from "./coordinator";
import type { parseArguments } from "@fig/autocomplete-parser";
const deferred = <T>() => {
  let resolve!: (value: T) => void;
  let reject!: (reason: Error) => void;
  const promise = new Promise<T>((a, b) => {
    resolve = a;
    reject = b;
  });
  return { promise, resolve, reject };
};
it("suppresses old results and errors after replacement or disposal", async () => {
  type Result = Awaited<ReturnType<typeof parseArguments>>;
  const first = deferred<Result>();
  const second = deferred<Result>();
  const parse = vi
    .fn<NonNullable<Parameters<typeof createParseCoordinator>[0]>>()
    .mockReturnValueOnce(first.promise)
    .mockReturnValueOnce(second.promise);
  const coordinator = createParseCoordinator(parse);
  const callbacks = { loading: vi.fn(), result: vi.fn(), error: vi.fn() };
  const context = {
    currentWorkingDirectory: "/",
    currentProcess: "zsh",
    environmentVariables: {},
    sshPrefix: "",
  };
  coordinator.run(null, context, callbacks);
  const dispose = coordinator.run(null, context, callbacks);
  first.resolve({} as Result);
  await Promise.resolve();
  expect(callbacks.result).not.toHaveBeenCalled();
  dispose();
  second.reject(new Error("stale"));
  await Promise.resolve();
  expect(callbacks.error).not.toHaveBeenCalled();
  expect(callbacks.loading).toHaveBeenCalledTimes(2);
});
it("accepts clearing the entire command without indexing an absent token", () => {
  expect(bufferChange(null, null)).toEqual({ backspaced: false, large: false });
});

it("publishes completion atomically without an intermediate loading=false callback", async () => {
  type Result = Awaited<ReturnType<typeof parseArguments>>;
  const callbacks = { loading: vi.fn(), result: vi.fn(), error: vi.fn() };
  const context = {
    currentWorkingDirectory: "/",
    currentProcess: "zsh",
    environmentVariables: {},
    sshPrefix: "",
  };
  const coordinator = createParseCoordinator(async () => ({}) as Result);
  coordinator.run(null, context, callbacks);
  await Promise.resolve();
  expect(callbacks.loading.mock.calls).toEqual([[true]]);
  expect(callbacks.result).toHaveBeenCalledOnce();
  const failure = createParseCoordinator(async () => {
    throw new Error("failed");
  });
  failure.run(null, context, callbacks);
  await Promise.resolve();
  expect(callbacks.loading.mock.calls).toEqual([[true], [true]]);
  expect(callbacks.error).toHaveBeenCalledOnce();
});
