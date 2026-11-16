import { readSettings } from "@fig/shared/settings";
import { vi } from "vitest";
import { createGeneratorCache, type GeneratorServices } from "../helpers";
export const cache = createGeneratorCache(readSettings({}));
export const services: GeneratorServices = {
  invoke: (fn) => fn(),
  execute: vi.fn(async () => ({ stdout: "a/\nx\nc/\nl", stderr: "", status: 0 })),
  cached: cache.runCachedGenerator,
  history: () => [],
  timeout: () => 5000,
};
