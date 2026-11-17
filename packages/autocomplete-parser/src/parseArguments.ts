import { withTimeout } from "@fig/shared/runtime-timing";
import {
  executeCommand,
  executeLoginShell,
  getSetting,
  isInDevMode,
} from "@fig/api-bindings/wrappers";
import { getSpecPath, loadSubcommandCached } from "./loadSpec";
import { createParser } from "./parserCore";
export * from "./parserCore";
export const defaultParser = createParser({
  withTimeout,
  specs: { getSpecPath, loadSubcommandCached },
  executeCommand,
  executeLoginShell,
  getSetting,
  isInDevMode,
});
export const { parseArguments, clearFigCaches } = defaultParser;
