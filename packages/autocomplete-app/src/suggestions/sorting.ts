import { makeArray } from "@fig/shared/utils";
import type { Suggestion } from "@fig/shared/internal";

// { command: { commandParam: count/lastUsedDate } }
// e.g. git: { add: 2, push: 4 }
type Index = Record<string, Record<string, number>>;
export type RecencyIndex = Readonly<Index>;

const RECENCY_KEY = "term_frequency_index_fig_only_recency";
const MAX_ENTRIES = 10_000;
const MAX_PER_COMMAND = 1_000;

export function createRecencyRepository(
  storage?: { getItem(key: string): string | null; setItem(key: string, value: string): void },
  now: () => number = Date.now,
) {
  let recencyIndex: Index = Object.create(null);
  const order = new Map<string, [string, string]>();
  const commands = new Map<string, Set<string>>();
  let timer: ReturnType<typeof setTimeout> | undefined;
  let dirty = false;
  const remove = (cmd: string, param: string) => {
    const values = { ...recencyIndex[cmd] };
    delete values[param];
    if (Object.keys(values).length) recencyIndex[cmd] = values;
    else delete recencyIndex[cmd];
    order.delete(JSON.stringify([cmd, param]));
    const names = commands.get(cmd);
    names?.delete(param);
    if (!names?.size) commands.delete(cmd);
  };
  const record = (cmd: string, param: string, time: number) => {
    const key = JSON.stringify([cmd, param]);
    order.delete(key);
    order.set(key, [cmd, param]);
    let names = commands.get(cmd);
    if (!names) {
      names = new Set();
      commands.set(cmd, names);
    }
    names.delete(param);
    names.add(param);
    recencyIndex = { ...recencyIndex, [cmd]: { ...recencyIndex[cmd], [param]: time } };
    if (names.size > MAX_PER_COMMAND) remove(cmd, names.values().next().value!);
    if (order.size > MAX_ENTRIES) remove(...order.values().next().value!);
  };
  try {
    const saved: unknown = JSON.parse(storage?.getItem(RECENCY_KEY) || "{}");
    if (saved && typeof saved === "object" && !Array.isArray(saved)) {
      const entries: [string, string, number][] = [];
      for (const [cmd, values] of Object.entries(saved)) {
        if (!values || typeof values !== "object" || Array.isArray(values)) continue;
        for (const [param, time] of Object.entries(values))
          if (typeof time === "number" && Number.isFinite(time)) entries.push([cmd, param, time]);
      }
      // Bulk construction avoids cloning an ever-growing index for every stored entry.
      const selected: [string, string, number][] = [];
      const counts = new Map<string, number>();
      for (const entry of entries.toSorted((a, b) => b[2] - a[2])) {
        const count = counts.get(entry[0]) ?? 0;
        if (count >= MAX_PER_COMMAND) continue;
        selected.push(entry);
        counts.set(entry[0], count + 1);
        if (selected.length === MAX_ENTRIES) break;
      }
      for (const [cmd, param, time] of selected.toReversed()) {
        recencyIndex[cmd] ??= Object.create(null);
        recencyIndex[cmd][param] = time;
        order.set(JSON.stringify([cmd, param]), [cmd, param]);
        let names = commands.get(cmd);
        if (!names) {
          names = new Set();
          commands.set(cmd, names);
        }
        names.add(param);
      }
    }
  } catch {
    /* Malformed or unavailable persistence is optional. */
  }

  const flush = () => {
    clearTimeout(timer);
    timer = undefined;
    if (!dirty) return;
    try {
      storage?.setItem(RECENCY_KEY, JSON.stringify(recencyIndex));
      dirty = false;
    } catch {
      /* Retry at the next flush; completion does not depend on storage. */
    }
  };
  const updateAutocompleteIndexFromUserInsert = (cmd: string, param: string) => {
    if (param.includes("↪")) return;
    record(cmd, param, now());
    dirty = true;
    timer ??= setTimeout(flush, 250);
  };
  return {
    getRecencyIndex: (): RecencyIndex => recencyIndex,
    updateAutocompleteIndexFromUserInsert,
    flush,
  };
}
/**
 * Pure: `sortMethod` is the `autocomplete.sortMethod` setting (default "recency") and
 * `lastInserted` the recency index to boost by (`getRecencyIndex()` in the app).
 */
export const updatePriorities = (
  suggestions: Suggestion[],
  cmd: string,
  sortMethod: unknown,
  lastInserted: RecencyIndex,
) => {
  const idxToUse = sortMethod !== "alphabetical" ? lastInserted : undefined;

  const cmdRecency = idxToUse && idxToUse[cmd];

  return suggestions.map((suggestion) => {
    const name = makeArray(suggestion.name)[0] || "";
    const recency = name !== "../" && cmdRecency && cmdRecency[name];
    const recencyBoost = recency ? recency / 10000000000000 : 0;

    let priority = suggestion.priority || 50;
    if (suggestion.type !== "auto-execute") {
      priority = Math.max(Math.min(100, priority), 0);
    }

    if (recency && priority >= 50 && priority <= 75) {
      priority = 75 + recencyBoost;
    } else {
      priority += recencyBoost;
    }
    return { ...suggestion, priority };
  });
};
