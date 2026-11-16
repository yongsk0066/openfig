import logger from "loglevel";

import { type GeneratorContext, haveContextForGenerator } from "./helpers";
import type { GeneratorServices } from "./helpers";

const transform = (result: Fig.Suggestion[]) =>
  result.map((name) => ({ ...name, type: name.type || "arg" }));

export async function getCustomSuggestions(
  generator: Fig.Generator,
  context: GeneratorContext,
  signal: AbortSignal | undefined,
  services: Pick<GeneratorServices, "execute" | "cached" | "invoke" | "listDirectory">,
  onRefresh?: (suggestions: Fig.Suggestion[]) => void,
): Promise<Fig.Suggestion[]> {
  if (!generator.custom) {
    return [];
  }

  if (!haveContextForGenerator(context)) {
    logger.info("Don't have context for custom generator");
    return [];
  }

  const {
    tokenArray,
    currentWorkingDirectory,
    currentProcess,
    isDangerous,
    searchTerm,
    environmentVariables,
  } = context;

  try {
    const result = await services.cached(
      generator,
      context,
      () =>
        services.invoke(() =>
          generator.custom!(tokenArray, (input) => services.execute(input, 5000, signal), {
            currentWorkingDirectory,
            currentProcess,
            sshPrefix: "",
            searchTerm,
            environmentVariables,
            isDangerous,
            listDirectory: services.listDirectory
              ? (path) => services.listDirectory!(path, signal)
              : undefined,
          }),
        ),
      generator.cache?.cacheKey,
      signal,
      onRefresh ? (value) => onRefresh(transform(value)) : undefined,
    );

    return transform(result);
  } catch (e) {
    if (signal?.aborted) throw signal.reason;
    logger.error("we had an error with the custom function generator", e);

    return [];
  }
}
