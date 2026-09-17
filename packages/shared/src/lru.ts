/**
 * A size-bounded cache with least-recently-used eviction, for module-level caches that
 * live as long as the autocomplete window (days).
 *
 * Two generations instead of a linked list: reads and writes land in `young`; when it
 * reaches half the capacity it becomes `old` and the previous `old` is dropped. A read
 * that finds a key in `old` moves it to `young`. The two maps never share a key, so
 * `size` is exact and never exceeds `maxSize`, and a hit in `young` costs one `Map#get`
 * (a true LRU pays a delete + re-insert on every hit, several thousand per keystroke for
 * fuzzy matching). Entries used within the last `maxSize / 2` writes always survive.
 */
export class LruCache<K, V> {
  readonly maxSize: number;

  private readonly generationSize: number;

  private young = new Map<K, V>();

  private old = new Map<K, V>();

  constructor(maxSize: number) {
    if (!Number.isInteger(maxSize) || maxSize < 2) {
      throw new RangeError(`LruCache maxSize must be an integer >= 2, got ${maxSize}`);
    }
    this.maxSize = maxSize;
    this.generationSize = Math.floor(maxSize / 2);
  }

  get size(): number {
    return this.young.size + this.old.size;
  }

  has(key: K): boolean {
    return this.young.has(key) || this.old.has(key);
  }

  get(key: K): V | undefined {
    const value = this.young.get(key);
    if (value !== undefined || this.young.has(key)) {
      return value;
    }
    if (!this.old.has(key)) {
      return undefined;
    }
    const oldValue = this.old.get(key) as V;
    this.old.delete(key);
    this.insert(key, oldValue);
    return oldValue;
  }

  set(key: K, value: V): this {
    if (this.young.has(key)) {
      this.young.set(key, value);
    } else {
      this.old.delete(key);
      this.insert(key, value);
    }
    return this;
  }

  delete(key: K): boolean {
    const inYoung = this.young.delete(key);
    const inOld = this.old.delete(key);
    return inYoung || inOld;
  }

  clear(): void {
    this.young.clear();
    this.old.clear();
  }

  *keys(): IterableIterator<K> {
    yield* this.old.keys();
    yield* this.young.keys();
  }

  private insert(key: K, value: V) {
    if (this.young.size >= this.generationSize) {
      this.old = this.young;
      this.young = new Map();
    }
    this.young.set(key, value);
  }
}
