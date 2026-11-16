import { executeCommand, fread, isInDevMode } from "@fig/api-bindings/wrappers";
import { createSpecLoader } from "./specLoader";
export {
  createSpecLoader,
  createSpecLoaderCaches,
  type SpecFileImport,
  type SpecLoaderPorts,
} from "./specLoader";
import { importSpecSource } from "./browser";
export const defaultSpecLoader = createSpecLoader({
  executeCommand: (input) => executeCommand(input),
  fread,
  isInDevMode,
  importSource: importSpecSource,
  importModule: (url) => import(/* @vite-ignore */ url),
  readJson: (url) => fetch(url).then((response) => response.json()),
});
export const {
  importString,
  importSpecFromFile,
  importBundledSpec,
  importFromLocalhost,
  getCachedCLIVersion,
  getVersionFromFullFile,
  clearSpecIndex,
  parseSpecIndex,
  bundledSpecExists,
  isDiffVersionedSpec,
  preloadSpecs,
} = defaultSpecLoader;
