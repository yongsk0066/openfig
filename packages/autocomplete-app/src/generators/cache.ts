import { LruCache } from "@fig/shared/lru";

class CacheEntry<T> {
  value: T | undefined;
  initialized = false;
  fetchedAt = 0;
  pending?: { promise: Promise<T>; signal?: AbortSignal };

  fetch(run: () => Promise<T>, signal?: AbortSignal): Promise<T> {
    if (this.pending) return this.pending.promise;
    const started = Date.now();
    const promise = Promise.resolve()
      .then(run)
      .then((value) => {
        if (!signal?.aborted) {
          this.value = value;
          this.initialized = true;
          this.fetchedAt = started;
        }
        return value;
      })
      .finally(() => {
        this.pending = undefined;
      });
    this.pending = { promise, signal };
    return promise;
  }
}

export class Cache {
  private cache = new LruCache<string, CacheEntry<unknown>>(500);

  async entry<T>(
    key: string,
    run: () => Promise<T>,
    cache: Fig.Cache,
    signal?: AbortSignal,
    onRefresh?: (value: T) => void,
  ): Promise<T> {
    signal?.throwIfAborted();
    let entry = this.cache.get(key);
    // In-flight work belongs to its cancellation scope. Carry over only committed data.
    if (
      !entry ||
      (entry.pending && (entry.pending.signal?.aborted || entry.pending.signal !== signal))
    ) {
      const replacement = new CacheEntry<unknown>();
      if (entry?.initialized) {
        replacement.value = entry.value;
        replacement.initialized = true;
        replacement.fetchedAt = entry.fetchedAt;
      }
      entry = replacement;
      this.cache.set(key, entry);
    }
    const current = entry;
    const expired = Date.now() > current.fetchedAt + (cache.ttl ?? 0);
    if (!current.initialized) return (await current.fetch(run, signal)) as T;
    if (!expired) return current.value as T;
    const fresh = current.fetch(run, signal) as Promise<T>;
    if (cache.strategy === "max-age") return fresh;
    void fresh
      .then((value) => {
        if (!signal?.aborted && this.cache.get(key) === current) onRefresh?.(value);
      })
      .catch(() => {
        /* Keep the committed stale value; retry on the next request. */
      });
    return current.value as T;
  }

  currentValue<T>(key: string, _cache: Fig.Cache): T | undefined {
    return this.cache.get(key)?.value as T | undefined;
  }
  get size() {
    return this.cache.size;
  }
  clear() {
    this.cache.clear();
  }
}
