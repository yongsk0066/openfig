import {
  createParserCaches,
  createSpecCatalogCaches,
  createSpecLoaderCaches,
} from "@fig/autocomplete-parser/core";
import { LruCache } from "@fig/shared/lru";
import { SETTINGS, type SettingsMap } from "@fig/shared/settings";
import type { Subcommand } from "@fig/shared/internal";

type Execution = { sessionId?: string; cwd?: string; env: Record<string, string | undefined> };
/** Catalog, execution facts and parsed specs have different invalidation boundaries. */
export function createCompletionCaches() {
  const catalog = createSpecCatalogCaches();
  const facts = new LruCache<string, ReturnType<typeof createSpecLoaderCaches>>(8);
  const parsed = new LruCache<
    string,
    {
      parser: ReturnType<typeof createParserCaches>;
      specs: LruCache<string, Subcommand>;
    }
  >(8);
  return {
    scope(execution: Execution, settings: SettingsMap) {
      const sourceSettings = [
        SETTINGS.DEV_MODE,
        SETTINGS.DEV_MODE_NPM,
        SETTINGS.DEV_COMPLETIONS_FOLDER,
        SETTINGS.DEV_COMPLETIONS_SERVER_PORT,
        SETTINGS.DEV_MODE_NPM_INVALIDATE_CACHE,
      ].map((setting) => settings[setting]);
      const context = JSON.stringify([
        execution.sessionId,
        execution.cwd,
        Object.entries(execution.env).toSorted(([a], [b]) => a.localeCompare(b)),
        sourceSettings,
      ]);
      let loader = facts.get(context);
      if (!loader) {
        loader = createSpecLoaderCaches(catalog);
        facts.set(context, loader);
      }
      const key = JSON.stringify([
        context,
        ...[
          SETTINGS.DISABLE_FOR_COMMANDS,
          SETTINGS.PERSONAL_SHORTCUTS_TOKEN,
          SETTINGS.FIRST_COMMAND_COMPLETION,
        ].map((setting) => settings[setting]),
      ]);
      let result = parsed.get(key);
      if (!result) {
        result = { parser: createParserCaches(), specs: new LruCache<string, Subcommand>(256) };
        parsed.set(key, result);
      }
      return { ...result, loader };
    },
    clear() {
      facts.clear();
      parsed.clear();
      catalog.stringImportCache.clear();
      catalog.publicSpecsRequest = undefined;
    },
  };
}
