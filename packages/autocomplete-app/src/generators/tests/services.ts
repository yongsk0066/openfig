import { runtimeTiming } from "@fig/shared/runtime-timing";
import { readSettings } from "@fig/shared/settings";
import { vi } from "vitest";
import { createGeneratorCache, type GeneratorServices } from "../helpers";
export const cache = createGeneratorCache(readSettings({}), () => Date.now());
export const services: GeneratorServices = {
  delay: runtimeTiming.delay,
  invoke: (fn) => fn(),
  execute: vi.fn(async () => ({ stdout: "a/\nx\nc/\nl", stderr: "", status: 0 })),
  cached: cache.runCachedGenerator,
  history: () => [],
  timeout: () => 5000,
};
