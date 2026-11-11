// The in-memory handles for an agent's `store Map`, `store Set`, `store Cache` and
// `store Log` fields (#3254).
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
// A `Log` is an append-ordered sequence, not a keyed collection: its handle appends,
// prunes past a retention cutoff, and reads time windows; a handle backed by storage
// reads a window as a key range instead of scanning.
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

  constructor(indexers: ReadonlyArray<Indexer<V>> = []) {
    this.indexers = indexers;
    for (const ix of indexers) this.postings.set(ix.index, new Map());
  }

  /** A handle holding `entries`, in the order given. */
  static from<V, M extends StoreMap<V> = StoreMap<V>>(
    this: new (indexers: readonly Indexer<V>[]) => M,
    entries: Iterable<readonly [string, V]>,
    indexers: readonly Indexer<V>[] = [],
  ): M {
    // `new this`, so a subclass (`StoreCache`) builds an instance of itself.
    const handle = new this(indexers);
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

  keys(): string[] {
    return [...this.slots.keys()];
  }

  values(): V[] {
    return [...this.slots.values()].map((s) => s.value);
  }

  entries(): Array<readonly [string, V]> {
    return [...this.slots].map(([k, s]) => [k, s.value] as const);
  }

  byIndex(index: string, by: string): V[] {
    const buckets = this.postings.get(index);
    // The indexes are fixed when the handle is built, so an unknown name is a lowering bug.
    if (buckets === undefined) throw new Error(`store map has no index "${index}"`);
    const pks = buckets.get(by) ?? [];
    return pks.map((pk) => (this.slots.get(pk) as Slot<V>).value);
  }

  /** A copy of the posting lists of `index`, as an encoded state record holds them. */
  postingsOf(index: string): ReadonlyMap<string, readonly string[]> {
    const buckets = this.postings.get(index) ?? new Map<string, string[]>();
    return new Map([...buckets].map(([by, pks]) => [by, [...pks]]));
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

  keys(): string[] {
    return [...this.members];
  }
}

/** A `store Cache` entry: the value, and the clock reading (ms) after which it is gone. */
export interface CacheEntry<V> {
  readonly v: V;
  readonly exp: number;
}

/** An agent's `store Cache`: a `Map` handle over `{ v, exp }`. */
export interface StoreCacheHandle<V> extends StoreMapHandle<CacheEntry<V>> {
  /** How many entries are live at `now` (their `exp` is after it). */
  liveSize(now: number): Awaitable<number>;
}

/** The eager, in-memory `store Cache` handle. Expiry is judged by the caller. */
export class StoreCache<V> extends StoreMap<CacheEntry<V>> implements StoreCacheHandle<V> {
  liveSize(now: number): number {
    let live = 0;
    for (const e of this.values()) if (e.exp > now) live += 1;
    return live;
  }
}

/** A `store Log` entry: the clock reading (ms) it was appended at, and the value. */
export interface LogEntry<V> {
  readonly t: number;
  readonly v: V;
}

/** An agent's `store Log`. Every read is in append order unless it says otherwise. */
export interface StoreLogHandle<V> {
  /** Append `value`, stamped `now`. */
  append(value: V, now: number): Awaitable<void>;
  /** Drop every entry appended before `cutoff` (`t < cutoff`), as `@retain` does. */
  prune(cutoff: number): Awaitable<void>;
  size(): Awaitable<number>;
  /** The values, oldest first. */
  values(): Awaitable<readonly V[]>;
  /** The stamped entries, oldest first. */
  entries(): Awaitable<ReadonlyArray<LogEntry<V>>>;
  /** The values appended at or after `t`. */
  since(t: number): Awaitable<readonly V[]>;
  /** The values appended before `t`. */
  before(t: number): Awaitable<readonly V[]>;
  /** The values appended from `from` to `to`, both inclusive. */
  between(from: number, to: number): Awaitable<readonly V[]>;
  /** The `n` most recent values, newest first. */
  recent(n: number): Awaitable<readonly V[]>;
  /** Every value, newest first. */
  reversed(): Awaitable<readonly V[]>;
}

/** The eager, in-memory `store Log` handle: the array the lowering indexes today. */
export class StoreLog<V> implements StoreLogHandle<V> {
  private items: LogEntry<V>[] = [];

  /** A handle holding `entries`, oldest first. */
  static from<V>(entries: Iterable<LogEntry<V>>): StoreLog<V> {
    const handle = new StoreLog<V>();
    handle.items = [...entries];
    return handle;
  }

  append(value: V, now: number): void {
    this.items.push({ t: now, v: value });
  }

  prune(cutoff: number): void {
    this.items = this.items.filter((e) => e.t >= cutoff);
  }

  size(): number {
    return this.items.length;
  }

  values(): V[] {
    return this.items.map((e) => e.v);
  }

  entries(): Array<LogEntry<V>> {
    return [...this.items];
  }

  since(t: number): V[] {
    return this.items.filter((e) => e.t >= t).map((e) => e.v);
  }

  before(t: number): V[] {
    return this.items.filter((e) => e.t < t).map((e) => e.v);
  }

  between(from: number, to: number): V[] {
    return this.items.filter((e) => e.t >= from && e.t <= to).map((e) => e.v);
  }

  recent(n: number): V[] {
    return this.items
      .slice(Math.max(0, this.items.length - Math.max(0, n)))
      .reverse()
      .map((e) => e.v);
  }

  reversed(): V[] {
    return [...this.items].reverse().map((e) => e.v);
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// The wire form (#3254). An agent's state is encoded to, and decoded from, a record
// shaped like the stored one: each `Map`/`Set`/`Cache` a `Record`, each `Log` an array.
// A `Record` cannot carry the insertion order of its integer-like keys (a JS object
// enumerates those first, by value), so a record built here also notes its key order
// beside it, and {@link entriesOf} reads it back in that order. A record that has no
// noted order (one read from a stored wholesale record, or built by a migration step)
// enumerates as an object does.

const recordOrders = new WeakMap<object, readonly string[]>();

/** A record holding `pairs`, with their order noted so {@link entriesOf} reads it back. */
export function orderedRecord<T>(pairs: Iterable<readonly [string, T]>): Record<string, T> {
  const record: Record<string, T> = {};
  const keys: string[] = [];
  for (const [k, v] of pairs) {
    Object.defineProperty(record, k, { value: v, enumerable: true, writable: true, configurable: true });
    keys.push(k);
  }
  recordOrders.set(record, keys);
  return record;
}

/** The entries of a wire record: in their noted insertion order, else as an object enumerates. */
export function entriesOf<T>(record: Readonly<Record<string, T>>): Array<[string, T]> {
  const order = recordOrders.get(record);
  if (order === undefined) return Object.entries(record);
  // The noted order first, then any key added since (a handler's write into the joined record).
  const noted = new Set(order);
  const keys = [...order.filter((k) => Object.hasOwn(record, k)), ...Object.keys(record).filter((k) => !noted.has(k))];
  return keys.map((k) => [k, record[k] as T]);
}

/** Note `order` as the insertion order of an existing wire record (a joined stored one). */
export function noteOrder(record: object, order: readonly string[]): void {
  recordOrders.set(record, order);
}

/**
 * The entries of a stored map-shaped field: a `Record`, or the pair array a held map is
 * stored as (a record cannot hold that map's order).
 */
export function wireEntries<T>(wire: Readonly<Record<string, T>> | ReadonlyArray<readonly [string, T]>): Array<[string, T]> {
  return Array.isArray(wire) ? wire.map(([k, v]) => [k, v] as [string, T]) : entriesOf(wire as Readonly<Record<string, T>>);
}

/** A `Map`'s wire record, its values passed through `f` if given. */
export function wireMap<V, W = V>(handle: StoreMap<V>, f?: (value: V) => W): Record<string, W> {
  return orderedRecord(handle.entries().map(([k, v]) => [k, f === undefined ? (v as unknown as W) : f(v)] as const));
}

/** A `Set`'s wire record: each member `true`. */
export function wireSet(handle: StoreSet): Record<string, boolean> {
  return orderedRecord(handle.keys().map((k) => [k, true] as const));
}

/** A `Log`'s wire array, its values passed through `f` if given. */
export function wireLog<V, W = V>(handle: StoreLog<V>, f?: (value: V) => W): Array<{ t: number; v: W }> {
  return handle.entries().map((e) => (f === undefined ? { ...e } : { ...e, v: f(e.v) })) as Array<{ t: number; v: W }>;
}

/** The posting lists of one `@indexed` field, as a wholesale record holds them. */
export function wirePostings<V>(handle: StoreMap<V>, index: string): Record<string, string[]> {
  return Object.fromEntries([...handle.postingsOf(index)].map(([by, pks]) => [by, [...pks]]));
}

/** A held `Map`'s wire form: key and connection id pairs, in insertion order. */
export function wireHeld<V>(handle: StoreMap<V>): Array<[string, V]> {
  return handle.entries().map(([k, v]) => [k, v] as [string, V]);
}

/** Fill an empty `Map` or `Cache` handle from its wire record, in wire order. */
export function fillMap<V>(handle: StoreMap<V>, wire: Readonly<Record<string, V>>): void {
  for (const [k, v] of entriesOf(wire)) handle.put(k, v);
}

/** Fill an empty `Set` handle from its wire record, in wire order. */
export function fillSet(handle: StoreSet, wire: Readonly<Record<string, unknown>>): void {
  for (const [k] of entriesOf(wire)) handle.add(k);
}

/** Fill an empty `Log` handle from its wire array. */
export function fillLog<V>(handle: StoreLog<V>, wire: ReadonlyArray<LogEntry<V>>): void {
  for (const e of wire) handle.append(e.v, e.t);
}

/** Fill an empty held `Map` handle from its stored pairs, or from an older stored record. */
export function fillHeld<V>(handle: StoreMap<V>, wire: Readonly<Record<string, V>> | ReadonlyArray<readonly [string, V]>): void {
  for (const [k, v] of wireEntries(wire)) handle.put(k, v);
}
