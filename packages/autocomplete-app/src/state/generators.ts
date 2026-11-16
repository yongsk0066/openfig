import logger from "loglevel";
import { sleep } from "@fig/shared/utils";
import { getTemplateSuggestions } from "../generators/templateSuggestionsGenerator";
import { getScriptSuggestions } from "../generators/scriptSuggestionsGenerator";
import { getCustomSuggestions } from "../generators/customSuggestionsGenerator";
import type { NamedSetState, AutocompleteState } from "./types";
import type { GeneratorState, GeneratorContext } from "../generators/helpers";
import { isTemplateSuggestion } from "../suggestions";
import type { RunGeneratorEffect } from "./effects";

import type { GeneratorServices } from "../generators/helpers";
const requestSuggestions = (
  { generator, context }: GeneratorState,
  signal: AbortSignal,
  services: GeneratorServices,
  onRefresh: (suggestions: Fig.Suggestion[]) => void,
) => {
  let request: Promise<Fig.Suggestion[]>;

  if (generator.template) {
    request = getTemplateSuggestions(generator, context, services.history);
  } else if (generator.script) {
    request = getScriptSuggestions(
      generator,
      context,
      services.timeout(),
      signal,
      services,
      onRefresh,
    );
  } else {
    const filter = (suggestions: Fig.Suggestion[]) =>
      generator.filterTemplateSuggestions && suggestions[0] && isTemplateSuggestion(suggestions[0])
        ? generator.filterTemplateSuggestions(suggestions as Fig.TemplateSuggestion[])
        : suggestions;
    request = getCustomSuggestions(generator, context, signal, services, (values) =>
      onRefresh(filter(values)),
    );
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
export const createGeneratorRunner = (
  setNamed: NamedSetState<AutocompleteState>,
  servicesFor: (context: GeneratorContext) => GeneratorServices,
) => {
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

  const active = new Map<GeneratorState, AbortController>();
  const reconcile = (states: readonly GeneratorState[]) => {
    for (const [state, controller] of active) {
      if (!states.includes(state)) {
        controller.abort();
        active.delete(state);
      }
    }
  };
  const runGenerator = ({ generatorState, debounceMs }: RunGeneratorEffect) => {
    const controller = new AbortController();
    active.set(generatorState, controller);
    let stored = generatorState;
    let initialized = false;
    let refreshed: Fig.Suggestion[] | undefined;
    const publish = (result: Fig.Suggestion[]) => {
      if (controller.signal.aborted) return;
      active.delete(stored);
      const updated = updateGenerator(stored, () => ({
        loading: false,
        result: result.map((suggestion) => ({
          ...suggestion,
          generator: generatorState.generator,
        })),
      }));
      if (updated) {
        stored = updated;
        active.set(stored, controller);
      } else controller.abort();
    };
    void (async () => {
      try {
        if (debounceMs !== undefined) await sleep(debounceMs);
        controller.signal.throwIfAborted();
        const result = await requestSuggestions(
          generatorState,
          controller.signal,
          servicesFor(generatorState.context),
          (values) => {
            if (initialized) publish(values);
            else refreshed = values;
          },
        );
        publish(result);
        initialized = true;
        if (refreshed) publish(refreshed);
      } catch (error) {
        active.delete(stored);
        if (!controller.signal.aborted) {
          logger.warn("Generator failed", error);
          updateGenerator(stored, () => ({ loading: false, result: [] }));
        }
      }
    })();
  };

  return { runGenerator, reconcile };
};

export { shellContextSelector, planGenerators } from "./generator-plan";
