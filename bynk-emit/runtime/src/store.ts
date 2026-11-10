// The in-memory handle for an agent's `store Map`, `store Set` and `store Cache`
// fields (#3254).
//
// A store collection is **insertion-ordered**: `keys()`, `values()` and `entries()`
// enumerate in the order the keys were first inserted, re-inserting an existing key
// keeps its position, and removing a key then inserting it again moves it to the
// end (the spec's `Map` order). A plain JS object cannot hold that order: it
// enumerates integer-like keys (`"0"`, `"5"`, `"10"`) in ascending numeric order
// before every other key, whatever order they went in. The handle is backed by a JS
// `Map`, which keeps insertion order for every key.
//
// Every method may return a promise (`Awaitable`), so a handle backed by storage
// can replace this eager one without the emitted code changing: the emitted code
// awaits each call. The eager handles here return plain values.
//
// A `Map` handle also keeps its `@indexed` postings: for each declared index, the
// primary keys whose value carries a given index value, in the entries' insertion
// order. `byIndex` reads a posting list instead of scanning every entry.

/** A value now or later; the emitted code awaits either. */
export type Awaitable<T> = T | Promise<T>;

/** One `@indexed(by:)` index: its name, and the index value a stored value carries. */
export interface Indexer<V> {
  readonly index: string;
  readonly of: (value: V) => string;
}

/** An agent's `store Map` (and, over `{ v, exp }`, its `store Cache`). */
export interface StoreMapHandle<V> {
  has(key: string): Awaitable<boolean>;
  get(key: string): Awaitable<V | undefined>;
  /** Insert or overwrite. An existing key keeps its position. Maintains the postings. */
  put(key: string, value: V): Awaitable<void>;
  /** Remove a key, if present, and its postings. Resolves to whether it was present. */
  remove(key: string): Awaitable<boolean>;
  size(): Awaitable<number>;
  /** The keys, in insertion order. */
  keys(): Awaitable<readonly string[]>;
  /** The values, in insertion order. */
  values(): Awaitable<readonly V[]>;
  /** The entries, in insertion order. */
  entries(): Awaitable<ReadonlyArray<readonly [string, V]>>;
  /** The values whose `index` value is `by`, in insertion order. */
  byIndex(index: string, by: string): Awaitable<readonly V[]>;
}

/** An agent's `store Set`. */
export interface StoreSetHandle {
  has(key: string): Awaitable<boolean>;
  /** Insert a member. An existing member keeps its position. */
  add(key: string): Awaitable<void>;
  /** Remove a member, if present. Resolves to whether it was present. */
  remove(key: string): Awaitable<boolean>;
  size(): Awaitable<number>;
  /** The members, in insertion order. */
  keys(): Awaitable<readonly string[]>;
}

interface Slot<V> {
  /** Insertion rank: a new key takes the next one, an overwrite keeps its own. */
  readonly rank: number;
  value: V;
}

/** The eager, in-memory `store Map` handle. */
export class StoreMap<V> implements StoreMapHandle<V> {
  private readonly slots = new Map<string, Slot<V>>();
  private readonly indexers: readonly Indexer<V>[];
  // index name -> index value -> primary keys in rank (insertion) order.
  private readonly postings = new Map<string, Map<string, string[]>>();
  private nextRank = 0;

  constructor(indexers: readonly Indexer<V>[] = []) {
    this.indexers = indexers;
    for (const ix of indexers) this.postings.set(ix.index, new Map());
  }

  /** A handle holding `entries`, in the order given. */
  static from<V>(entries: Iterable<readonly [string, V]>, indexers: readonly Indexer<V>[] = []): StoreMap<V> {
    const handle = new StoreMap<V>(indexers);
    for (const [k, v] of entries) handle.put(k, v);
    return handle;
  }

  has(key: string): boolean {
    return this.slots.has(key);
  }

  get(key: string): V | undefined {
    return this.slots.get(key)?.value;
  }

  put(key: string, value: V): void {
    const existing = this.slots.get(key);
    if (existing === undefined) {
      const slot: Slot<V> = { rank: this.nextRank, value };
      this.nextRank += 1;
      this.slots.set(key, slot);
      for (const ix of this.indexers) this.addPosting(ix, key, slot);
      return;
    }
    // An overwrite keeps its position, and moves between posting lists only if the
    // value it is indexed under changed.
    const old = existing.value;
    existing.value = value;
    for (const ix of this.indexers) {
      const from = ix.of(old);
      const to = ix.of(value);
      if (from === to) continue;
      this.dropPosting(ix, from, key);
      this.addPosting(ix, key, existing);
    }
  }

  remove(key: string): boolean {
    const slot = this.slots.get(key);
    if (slot === undefined) return false;
    for (const ix of this.indexers) this.dropPosting(ix, ix.of(slot.value), key);
    this.slots.delete(key);
    return true;
  }

  size(): number {
    return this.slots.size;
  }

  keys(): readonly string[] {
    return [...this.slots.keys()];
  }

  values(): readonly V[] {
    return [...this.slots.values()].map((s) => s.value);
  }

  entries(): ReadonlyArray<readonly [string, V]> {
    return [...this.slots].map(([k, s]) => [k, s.value] as const);
  }

  byIndex(index: string, by: string): readonly V[] {
    const pks = this.postings.get(index)?.get(by) ?? [];
    return pks.map((pk) => (this.slots.get(pk) as Slot<V>).value);
  }

  /** The posting lists of `index`, as an encoded state record holds them. */
  postingsOf(index: string): ReadonlyMap<string, readonly string[]> {
    return this.postings.get(index) ?? new Map();
  }

  private addPosting(ix: Indexer<V>, key: string, slot: Slot<V>): void {
    const by = ix.of(slot.value);
    const buckets = this.postings.get(ix.index) as Map<string, string[]>;
    const bucket = buckets.get(by) ?? [];
    buckets.set(by, bucket);
    // A posting list is in insertion order: the entry goes after every member that was
    // inserted before it. A new key is the newest, so that is the end; a key whose
    // indexed value changed is older than some members, so it goes in at its rank.
    let at = bucket.length;
    while (at > 0 && (this.slots.get(bucket[at - 1] as string) as Slot<V>).rank > slot.rank) at -= 1;
    bucket.splice(at, 0, key);
  }

  private dropPosting(ix: Indexer<V>, by: string, key: string): void {
    const buckets = this.postings.get(ix.index) as Map<string, string[]>;
    const bucket = buckets.get(by);
    if (bucket === undefined) return;
    const at = bucket.indexOf(key);
    if (at >= 0) bucket.splice(at, 1);
    if (bucket.length === 0) buckets.delete(by);
  }
}

/** The eager, in-memory `store Set` handle. */
export class StoreSet implements StoreSetHandle {
  private readonly members = new Set<string>();

  /** A handle holding `keys`, in the order given. */
  static from(keys: Iterable<string>): StoreSet {
    const handle = new StoreSet();
    for (const k of keys) handle.add(k);
    return handle;
  }

  has(key: string): boolean {
    return this.members.has(key);
  }

  add(key: string): void {
    this.members.add(key);
  }

  remove(key: string): boolean {
    return this.members.delete(key);
  }

  size(): number {
    return this.members.size;
  }

  keys(): readonly string[] {
    return [...this.members];
  }
}
