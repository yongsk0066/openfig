import type { Subcommand } from "@fig/shared/internal";
import { LruCache } from "@fig/shared/lru";

const allCaches: Array<LruCache<string, unknown>> = [];

/**
 * Every parser cache is bounded: the autocomplete window stays open for days, and keys
 * such as the parse cache's (tokens, spec, cwd, shell) never repeat once the user moves on.
 */
export const createCache = <T>(maxSize = 1000) => {
  const cache = new LruCache<string, T>(maxSize);
  allCaches.push(cache as LruCache<string, unknown>);
  return cache;
};

export const resetCaches = () => {
  allCaches.forEach((cache) => {
    cache.clear();
  });
};

window.resetCaches = resetCaches;

// Converted specs, one per (source, path, name); the ~30 preloaded specs fit many times over.
export const specCache = createCache<Subcommand>(256);
export const generateSpecCache = createCache<Subcommand>(256);

window.listCache = () => {
  console.log(specCache);
  console.log(generateSpecCache);
};
