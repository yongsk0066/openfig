import { withTimeout } from "@fig/shared/runtime-timing";
import { Settings } from "@fig/api-bindings";
import { executeCommand, getSetting, isInDevMode } from "@fig/api-bindings/wrappers";
import * as loader from "./loadHelpers";
import { createSpecRepository } from "./specRepository";
export {
  createSpecRepository,
  serializeSpecLocation,
  type SpecRepositoryPorts,
} from "./specRepository";
export const defaultSpecRepository = createSpecRepository({
  withTimeout,
  loader: { ...loader, clear: () => loader.defaultSpecLoader.clear() },
  executeCommand,
  getSetting,
  isInDevMode,
  setSetting: Settings.set,
});
export const { getSpecPath, importSpecFromLocation, loadFigSubcommand, loadSubcommandCached } =
  defaultSpecRepository;
