// `@fig/autocomplete-helpers`: versioned-spec diffing, used by the specs themselves.
export {
  applySpecDiff,
  createVersionedSpec,
  diffSpecs,
  getVersionFromVersionedSpec,
} from "./versions";

// `@fig/autocomplete-shared`: the internal spec representation the parser and the
// app work with. Kept under the same package because the two never collide and
// both are Fig-side spec plumbing.
export type * as Internal from "./convert";
export type * as Metadata from "./specMetadata";
export { convertSubcommand } from "./convert";
export { revertSubcommand } from "./revert";
export { convertLoadSpec, initializeDefault } from "./specMetadata";
export { applyMixin, mergeSubcommands } from "./mixins";
export type { SpecMixin } from "./mixins";
export { makeArray, SpecLocationSource } from "./utils";
