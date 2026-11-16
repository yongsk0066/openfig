import type { ArgumentParserResult } from "@fig/autocomplete-parser/core";
import type { AutocompleteState } from "./types";
import type { GeneratorState, GeneratorContext } from "../generators/helpers";
import type { RunGeneratorEffect } from "./effects";
export const shellContextSelector = ({
  figState,
}: AutocompleteState): Fig.ShellContext & { sessionId?: string; inputRevision?: bigint } => ({
  sessionId: figState.shellContext?.sessionId,
  inputRevision: figState.shellContext?.inputRevision,
  currentWorkingDirectory: figState.cwd || "",
  currentProcess: figState.processUserIsIn || "",
  environmentVariables: figState.environmentVariables,
  sshPrefix: "",
});

const getGeneratorContext = (state: AutocompleteState): GeneratorContext => {
  const { command, parserResult } = state;
  const { currentArg, searchTerm, annotations, commandIndex } = parserResult;
  const tokens = command?.tokens ?? [];
  return {
    ...shellContextSelector(state),
    annotations: annotations.slice(commandIndex),
    tokenArray: tokens.slice(commandIndex).map((token) => token.text),
    isDangerous: Boolean(currentArg?.isDangerous),
    searchTerm,
  };
};

/**
 * Which generators of the new parser result must (re)run. Returns the generator states to
 * store — untouched ones keep their identity — and one `runGenerator` effect per generator
 * to start. Starts nothing itself. (A spec's `trigger` function is called here: it is spec
 * code, but only compares two search terms.)
 */
export const planGenerators = (
  state: AutocompleteState,
  parserResult: ArgumentParserResult,
): { generatorStates: GeneratorState[]; effects: RunGeneratorEffect[] } => {
  const {
    parserResult: { currentArg: previousArg, searchTerm: previousSearchTerm },
  } = state;
  const { currentArg, searchTerm } = parserResult;
  const generators = currentArg?.generators ?? [];
  const context = getGeneratorContext({ ...state, parserResult });
  const effects: RunGeneratorEffect[] = [];

  const generatorStates = generators.map((generator, index) => {
    const { trigger } = generator;
    const previousGeneratorState = state.generatorStates[index];
    let shouldTrigger = false;
    if (!previousGeneratorState || currentArg !== previousArg) {
      shouldTrigger = true;
    } else if (trigger === undefined) {
      // If trigger is undefined we never trigger, unless debounced in
      // which case we always trigger.
      // TODO: move debounce to generator.
      shouldTrigger = Boolean(currentArg?.debounce);
    } else {
      let triggerFn: (a: string, b: string) => boolean;
      if (typeof trigger === "string") {
        triggerFn = (a, b) => a.lastIndexOf(trigger) !== b.lastIndexOf(trigger);
      } else if (typeof trigger === "function") {
        triggerFn = trigger;
      } else {
        switch (trigger.on) {
          case "threshold": {
            triggerFn = (a, b) => a.length > trigger.length && !(b.length > trigger.length);
            break;
          }
          case "match": {
            const strings = typeof trigger.string === "string" ? [trigger.string] : trigger.string;
            triggerFn = (a, b) =>
              strings.findIndex((x) => x === a) !== strings.findIndex((x) => x === b);
            break;
          }
          case "change":
          default: {
            triggerFn = (a, b) => a !== b;
            break;
          }
        }
      }
      try {
        shouldTrigger = triggerFn(searchTerm, previousSearchTerm);
      } catch (_err) {
        shouldTrigger = true;
      }
    }

    if (!shouldTrigger) {
      return previousGeneratorState;
    }

    const result = previousGeneratorState?.result || [];
    const generatorState: GeneratorState = { generator, context, result, loading: true };

    if (currentArg?.debounce) {
      effects.push({
        kind: "runGenerator",
        generatorState,
        debounceMs:
          typeof currentArg.debounce === "number" && currentArg.debounce > 0
            ? currentArg.debounce
            : 200,
      });
    } else {
      effects.push({ kind: "runGenerator", generatorState });
    }
    return generatorState;
  });

  return { generatorStates, effects };
};
