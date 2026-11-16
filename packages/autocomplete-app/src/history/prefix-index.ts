/** Immutable-source index; ownership follows the entries array via WeakMap. */
export function createPrefixIndex<T extends { text: string }>() {
  const indexes = new WeakMap<readonly T[], { sorted: number[]; cache: Map<string, T[]> }>();
  return (entries: readonly T[], prefix: string): readonly T[] => {
    if (!prefix) return entries;
    let index = indexes.get(entries);
    if (!index) {
      index = {
        sorted: entries
          .map((_, i) => i)
          .toSorted((a, b) =>
            entries[a].text < entries[b].text ? -1 : entries[a].text > entries[b].text ? 1 : a - b,
          ),
        cache: new Map(),
      };
      indexes.set(entries, index);
    }
    const cached = index.cache.get(prefix);
    if (cached) return cached;
    let low = 0,
      high = index.sorted.length;
    while (low < high) {
      const mid = (low + high) >>> 1;
      if (entries[index.sorted[mid]].text < prefix) low = mid + 1;
      else high = mid;
    }
    const matches: number[] = [];
    for (let i = low; i < index.sorted.length; i++) {
      const id = index.sorted[i];
      if (!entries[id].text.startsWith(prefix)) break;
      matches.push(id);
    }
    const result = matches.toSorted((a, b) => a - b).map((i) => entries[i]);
    if (index.cache.size >= 16) index.cache.delete(index.cache.keys().next().value!);
    index.cache.set(prefix, result);
    return result;
  };
}
