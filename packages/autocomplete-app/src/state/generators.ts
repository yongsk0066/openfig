import logger from "loglevel";
import { sleep } from "@fig/shared/utils";
import type { ArgumentParserResult } from "@fig/autocomplete-parser";
import { SETTINGS, getSetting } from "@fig/api-bindings/wrappers";
import { runPipingConsoleMethods } from "../utils";
import { getTemplateSuggestions } from "../generators/templateSuggestionsGenerator";
import { getScriptSuggestions } from "../generators/scriptSuggestionsGenerator";
import { getCustomSuggestions } from "../generators/customSuggestionsGenerator";
import type { NamedSetState, AutocompleteState } from "./types";
import type { GeneratorState, GeneratorContext } from "../generators/helpers";
import { isTemplateSuggestion } from "../suggestions";
import type { RunGeneratorEffect } from "./effects";

export const shellContextSelector = ({ figState }: AutocompleteState): Fig.ShellContext => ({
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
        runPipingConsoleMethods(() => {
          shouldTrigger = triggerFn(searchTerm, previousSearchTerm);
        });
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

const requestSuggestions = ({ generator, context }: GeneratorState) => {
  let request: Promise<Fig.Suggestion[]>;

  if (generator.template) {
    request = getTemplateSuggestions(generator, context);
  } else if (generator.script) {
    request = getScriptSuggestions(
      generator,
      context,
      getSetting<number>(SETTINGS.SCRIPT_TIMEOUT, 5000),
    );
  } else {
    request = getCustomSuggestions(generator, context);
    // filepaths/folders templates are now a sugar for two custom generators, we need to filter
    // the suggestion created by those two custom generators
    if (generator.filterTemplateSuggestions) {
      request = (async () => {
        // TODO: use symbols to detect if the the generator fn is filepaths/folders
        // If the first custom suggestion has template meta properties then all the custom
        // suggestions are too
        const suggestions = await request;
        if (suggestions[0] && isTemplateSuggestion(suggestions[0])) {
          return generator.filterTemplateSuggestions!(suggestions as Fig.TemplateSuggestion[]);
        }
        return suggestions;
      })();
    }
  }
  return request;
};

/** Carries out `runGenerator` effects: runs the generator and stores its result. */
export const createGeneratorRunner = (setNamed: NamedSetState<AutocompleteState>) => {
  /** Replaces `stored` with `getUpdate()` unless another parse has replaced it already. */
  function updateGenerator(
    stored: GeneratorState,
    getUpdate: () => Partial<GeneratorState>,
  ): GeneratorState | undefined {
    let updated: GeneratorState | undefined;
    setNamed("updateGenerator", (state) => {
      let { generatorStates } = state;
      // Double check to make sure we don't update if things are stale
      const index = generatorStates.findIndex((s) => s === stored);
      if (index === -1) {
        logger.info("stale update", { generatorStates, generatorState: stored });
        return { generatorStates };
      }

      generatorStates = [...generatorStates];
      updated = { ...stored, ...getUpdate() };
      generatorStates[index] = updated;
      logger.info("updating generator", { generatorState: updated });
      return { generatorStates };
    });
    return updated;
  }

  const start = (running: GeneratorState) => {
    logger.info("Triggering generator", { currentState: running });
    const { generator } = running;
    void requestSuggestions(running).then((result) =>
      updateGenerator(running, () => ({
        loading: false,
        result: result.map((suggestion) => ({ ...suggestion, generator })),
      })),
    );
  };

  const runGenerator = ({ generatorState, debounceMs }: RunGeneratorEffect) => {
    if (debounceMs === undefined) {
      start(generatorState);
      return;
    }
    void sleep(debounceMs).then(() => {
      // Still current after the debounce: store a fresh loading state, then start it.
      const running = updateGenerator(generatorState, () => ({ loading: true }));
      if (running) start(running);
    });
  };

  return { runGenerator };
};
