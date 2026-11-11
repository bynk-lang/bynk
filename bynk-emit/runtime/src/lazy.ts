// The storage-backed handles for an agent's `store Map`, `store Set`, `store Cache` and
// `store Log` fields (#3189, the lazy-read slice).
//
// They implement the same handle interfaces as the in-memory ones in `store.ts`, but read an
// entry, a posting list or a key range from storage when the handler asks for it, and stage
// their writes in the handler's `EntryOverlay`, which flushes them (with the `"state"` record
// and each collection's metadata) in one transaction. Nothing here changes a stored byte: the
// keys and values are exactly what `entries.ts` writes (`m/ s/ l/ i/ x/ c/`).
//
// A handler's storage I/O follows what it touches: `get`, `has`, `put` and `remove` of one key
// read one entry, an `@indexed` lookup lists one posting prefix and reads the matched entries
// in batches, and a `Log` window is a key range. A scan (`keys`, `values`, `entries`) lists the
// collection in pages. Order is the entry's insertion sequence `i`, as `entries.ts` writes it.

import { EntryOverlay, setLazyContext, decodeKeyPart, expiredRange, expiryKey, logKey, logPrefix, logRange, mapKey, mapPrefix, metaKey, parseExpiryKey, parseLogKey, postingKey, postingPrefix, sameStored, setKey, setPrefix } from "./entries.ts";
import type { DurableObjectListOptions, DurableObjectStorage } from "./storage.ts";
import type { StoreMap } from "./store.ts";
import type { CacheEntry, Indexer, LogEntry, StoreCacheHandle, StoreLogHandle, StoreMapHandle, StoreSetHandle } from "./store.ts";

/** How many keys one page of a scan lists. */
export const SCAN_PAGE = 512;

interface Envelope {
  readonly i: number;
}

/** A collection's metadata record, `c/<field>`. */
interface Meta {
  size: number;
  seq: number;
  /**
   * A `Log`'s largest key time beyond an entry's own `t` (its keys never go backwards). Absent
   * means *unknown*, not zero: a record written before the field existed, or by a build that
   * drops it, may hold clamped entries. An unknown skew is never trusted to bound a window.
   */
  skew: number | undefined;
}

// Every key of a range, in key order, in pages (a list returns everything it matches).
async function listAll<T>(overlay: EntryOverlay, options: DurableObjectListOptions): Promise<Array<[string, T]>> {
  const out: Array<[string, T]> = [];
  if (options.reverse === true) {
    const all = await overlay.list<T>(options);
    return [...all];
  }
  let after: string | undefined;
  for (;;) {
    const page = await overlay.list<T>({ ...options, limit: SCAN_PAGE, ...(after === undefined ? {} : { startAfter: after }) });
    for (const [k, v] of page) out.push([k, v]);
    if (page.size < SCAN_PAGE) return out;
    after = out[out.length - 1]![0];
  }
}

// Runs the steps of one key one after another. A step reads what it is about to change, so two on the
// same key would each read the old state; steps on different keys do not wait for each other.
class KeyChains {
  private readonly tails = new Map<string, Promise<unknown>>();

  run<T>(key: string, step: () => Promise<T>): Promise<T> {
    const prior = this.tails.get(key) ?? Promise.resolve();
    const result = prior.then(step, step);
    const tail = result.then(
      () => undefined,
      () => undefined,
    );
    this.tails.set(key, tail);
    void tail.then(() => {
      if (this.tails.get(key) === tail) this.tails.delete(key);
    });
    return result;
  }
}

class MetaCell {
  private value: Meta = { size: 0, seq: 0, skew: undefined };
  private loading: Promise<Meta> | undefined;
  private existed = false;
  private changed = false;
  private readonly overlay: EntryOverlay;
  private readonly key: string;

  constructor(overlay: EntryOverlay, field: string) {
    this.overlay = overlay;
    this.key = metaKey(field);
  }

  load(): Promise<Meta> {
    // Memoised: a handler's concurrent operations share one read, and one `Meta` they all update.
    this.loading ??= (async () => {
      const stored = await this.overlay.get<Partial<Meta>>(this.key);
      this.existed = stored !== undefined;
      // A collection with no metadata at all is empty: it has nothing clamped, so its skew is known to be 0.
      this.value = { size: stored?.size ?? 0, seq: stored?.seq ?? 0, skew: stored === undefined ? 0 : typeof stored.skew === "number" ? stored.skew : undefined };
      return this.value;
    })();
    return this.loading;
  }

  touch(): void {
    this.changed = true;
  }

  /** Stage the metadata if this handler changed it (and it is worth writing, as `commitStored` has it). */
  finish(): void {
    if (!this.changed) return;
    const m = this.value;
    if (m.size > 0 || this.existed) {
      this.overlay.put(this.key, { size: m.size, seq: m.seq, ...(m.skew !== undefined ? { skew: m.skew } : {}) });
    }
    this.changed = false;
  }
}

/** How a map-shaped handle reads and writes its entries' stored envelope. */
export interface MapEnvelope<V> {
  /** The domain value an envelope holds (decoding the stored wire value). */
  read(envelope: never): V;
  /** The envelope for `value` at insertion sequence `i`. */
  write(value: V, i: number): unknown;
  /** The expiry an envelope carries, for a `Cache`. */
  expiryOf?(envelope: never): number | undefined;
}

/** The storage-backed `store Map` handle (and, with `expiryOf`, the `store Cache` one). */
export class LazyStoreMap<V> implements StoreMapHandle<V> {
  /** Marks a storage-backed handle: it holds no record a wire encoder could carry (`store.ts`). */
  readonly lazy = true;
  protected readonly overlay: EntryOverlay;
  protected readonly field: string;
  protected readonly meta: MetaCell;
  private readonly chains = new KeyChains();
  private readonly envelope: MapEnvelope<V>;
  private readonly indexers: readonly Indexer<V>[];

  constructor(overlay: EntryOverlay, field: string, envelope: MapEnvelope<V>, indexers: ReadonlyArray<Indexer<V>> = []) {
    this.overlay = overlay;
    this.field = field;
    this.envelope = envelope;
    this.indexers = indexers;
    this.meta = new MetaCell(overlay, field);
  }

  private env(raw: unknown): V {
    return this.envelope.read(raw as never);
  }

  private expiry(raw: unknown): number | undefined {
    return this.envelope.expiryOf?.(raw as never);
  }

  async has(key: string): Promise<boolean> {
    return (await this.overlay.get(mapKey(this.field, key))) !== undefined;
  }

  async get(key: string): Promise<V | undefined> {
    const raw = await this.overlay.get(mapKey(this.field, key));
    return raw === undefined ? undefined : this.env(raw);
  }

  put(key: string, value: V): Promise<void> {
    return this.chains.run(key, () => this.putNow(key, value));
  }

  modify(key: string, f: (current: V | undefined, present: boolean) => V): Promise<void> {
    return this.chains.run(key, async () => {
      const raw = await this.overlay.get(mapKey(this.field, key));
      const present = raw !== undefined;
      await this.putNow(key, f(present ? this.env(raw) : undefined, present));
    });
  }

  private async putNow(key: string, value: V): Promise<void> {
    const mk = mapKey(this.field, key);
    const old = await this.overlay.get<Envelope>(mk);
    const meta = await this.meta.load();
    let i: number;
    const written = (n: number): unknown => this.envelope.write(value, n);
    if (old === undefined) {
      // A new key (or one removed and inserted again) takes the next insertion sequence.
      i = meta.seq;
      meta.seq += 1;
      meta.size += 1;
      this.meta.touch();
      for (const ix of this.indexers) this.overlay.put(postingKey(this.field, ix.index, ix.of(value), key), 1);
      const exp = this.expiry(written(i));
      if (exp !== undefined) this.overlay.put(expiryKey(this.field, exp, key), 1);
      this.overlay.put(mk, written(i));
      return;
    }
    i = old.i;
    const next = written(i);
    if (this.indexers.length > 0) {
      const before = this.env(old);
      for (const ix of this.indexers) {
        const from = ix.of(before);
        const to = ix.of(value);
        if (from === to) continue;
        this.overlay.delete(postingKey(this.field, ix.index, from, key));
        this.overlay.put(postingKey(this.field, ix.index, to, key), 1);
      }
    }
    const was = this.expiry(old);
    const now = this.expiry(next);
    if (was !== now) {
      if (was !== undefined) this.overlay.delete(expiryKey(this.field, was, key));
      if (now !== undefined) this.overlay.put(expiryKey(this.field, now, key), 1);
    }
    if (!sameStored(old, next)) this.overlay.put(mk, next);
  }

  remove(key: string): Promise<boolean> {
    return this.chains.run(key, () => this.removeNow(key));
  }

  private async removeNow(key: string): Promise<boolean> {
    const mk = mapKey(this.field, key);
    const old = await this.overlay.get<Envelope>(mk);
    if (old === undefined) return false;
    const meta = await this.meta.load();
    meta.size -= 1;
    this.meta.touch();
    this.overlay.delete(mk);
    if (this.indexers.length > 0) {
      const before = this.env(old);
      for (const ix of this.indexers) this.overlay.delete(postingKey(this.field, ix.index, ix.of(before), key));
    }
    const exp = this.expiry(old);
    if (exp !== undefined) this.overlay.delete(expiryKey(this.field, exp, key));
    return true;
  }

  async size(): Promise<number> {
    return (await this.meta.load()).size;
  }

  // Every entry, by insertion sequence.
  private async scan(): Promise<Array<[string, Envelope]>> {
    const prefix = mapPrefix(this.field);
    const all = await listAll<Envelope>(this.overlay, { prefix });
    return all.map(([k, e]): [string, Envelope] => [decodeKeyPart(k.slice(prefix.length)), e]).sort((a, b) => a[1].i - b[1].i);
  }

  async keys(): Promise<readonly string[]> {
    return (await this.scan()).map(([k]) => k);
  }

  async values(): Promise<readonly V[]> {
    return (await this.scan()).map(([, e]) => this.env(e));
  }

  async entries(): Promise<ReadonlyArray<readonly [string, V]>> {
    return (await this.scan()).map(([k, e]) => [k, this.env(e)] as const);
  }

  async byIndex(index: string, by: string): Promise<readonly V[]> {
    if (!this.indexers.some((ix) => ix.index === index)) throw new Error(`store map has no index "${index}"`);
    const prefix = postingPrefix(this.field, index, by);
    const postings = await listAll<unknown>(this.overlay, { prefix });
    const pks = postings.map(([k]) => decodeKeyPart(k.slice(prefix.length)));
    const found = await this.overlay.getMany<Envelope>(pks.map((pk) => mapKey(this.field, pk)));
    return [...found.values()].sort((a, b) => a.i - b.i).map((e) => this.env(e));
  }

  /** Stage this handler's metadata. The commit calls it once, before the flush. */
  finish(): void {
    this.meta.finish();
  }
}

/** The storage-backed `store Cache` handle: a map over `{ v, exp }`, plus expiry. */
export class LazyStoreCache<V> extends LazyStoreMap<CacheEntry<V>> implements StoreCacheHandle<V> {
  async liveSize(now: number): Promise<number> {
    const expired = await listAll<unknown>(this.overlay, expiredRange(this.field, now));
    return (await this.size()) - expired.length;
  }

  /**
   * Delete up to `limit` entries that expired at or before `now`, oldest expiry first, and
   * resolve to how many it deleted. Reads only the expiry range, not the collection.
   */
  async reap(now: number, limit: number): Promise<number> {
    if (limit <= 0) return 0;
    const expired = await this.overlay.list<unknown>({ ...expiredRange(this.field, now), limit });
    let reaped = 0;
    for (const xk of expired.keys()) {
      const { key } = parseExpiryKey(this.field, xk);
      if (await this.remove(key)) reaped += 1;
      // An expiry key whose entry is gone (or was rewritten under another expiry) is an orphan:
      // drop it, or every reap would spend its limit on it again.
      this.overlay.delete(xk);
    }
    return reaped;
  }
}

/** The storage-backed `store Set` handle. */
export class LazyStoreSet implements StoreSetHandle {
  /** Marks a storage-backed handle: it holds no record a wire encoder could carry (`store.ts`). */
  readonly lazy = true;
  private readonly overlay: EntryOverlay;
  private readonly field: string;
  private readonly meta: MetaCell;
  private readonly chains = new KeyChains();

  constructor(overlay: EntryOverlay, field: string) {
    this.overlay = overlay;
    this.field = field;
    this.meta = new MetaCell(overlay, field);
  }

  async has(key: string): Promise<boolean> {
    return (await this.overlay.get(setKey(this.field, key))) !== undefined;
  }

  add(key: string): Promise<void> {
    return this.chains.run(key, () => this.addNow(key));
  }

  private async addNow(key: string): Promise<void> {
    const sk = setKey(this.field, key);
    if ((await this.overlay.get(sk)) !== undefined) return;
    const meta = await this.meta.load();
    this.overlay.put(sk, { i: meta.seq });
    meta.seq += 1;
    meta.size += 1;
    this.meta.touch();
  }

  remove(key: string): Promise<boolean> {
    return this.chains.run(key, () => this.removeNow(key));
  }

  private async removeNow(key: string): Promise<boolean> {
    const sk = setKey(this.field, key);
    if ((await this.overlay.get(sk)) === undefined) return false;
    const meta = await this.meta.load();
    meta.size -= 1;
    this.meta.touch();
    this.overlay.delete(sk);
    return true;
  }

  async size(): Promise<number> {
    return (await this.meta.load()).size;
  }

  async keys(): Promise<readonly string[]> {
    const prefix = setPrefix(this.field);
    const all = await listAll<Envelope>(this.overlay, { prefix });
    return all.sort((a, b) => a[1].i - b[1].i).map(([k]) => decodeKeyPart(k.slice(prefix.length)));
  }

  finish(): void {
    this.meta.finish();
  }
}

/** How a `Log` handle decodes and encodes an entry's value. */
export interface LogCodec<V> {
  decode(wire: never): V;
  encode(value: V): unknown;
}

interface StoredLogEntry {
  readonly t: number;
  readonly v: unknown;
}

/** The storage-backed `store Log` handle. */
export class LazyStoreLog<V> implements StoreLogHandle<V> {
  /** Marks a storage-backed handle: it holds no record a wire encoder could carry (`store.ts`). */
  readonly lazy = true;
  private readonly overlay: EntryOverlay;
  private readonly field: string;
  private readonly meta: MetaCell;
  private readonly codec: LogCodec<V>;
  private lastKeyTime: number | undefined;
  private readonly chains = new KeyChains();

  constructor(overlay: EntryOverlay, field: string, codec: LogCodec<V>) {
    this.overlay = overlay;
    this.field = field;
    this.codec = codec;
    this.meta = new MetaCell(overlay, field);
  }

  private entry(e: StoredLogEntry): LogEntry<V> {
    return { t: e.t, v: this.codec.decode(e.v as never) };
  }

  // The skew of a log whose record does not say: the largest key time above an entry's `t`, found by
  // reading the log once. A writer establishes it (so it is recorded); a reader does not write, and
  // widens its window without bound instead.
  private async establishSkew(): Promise<Meta> {
    const meta = await this.meta.load();
    if (meta.skew !== undefined) return meta;
    let skew = 0;
    for (const [k, e] of await listAll<StoredLogEntry>(this.overlay, { prefix: logPrefix(this.field) })) {
      skew = Math.max(skew, parseLogKey(k).t - e.t);
    }
    meta.skew = skew;
    this.meta.touch();
    return meta;
  }

  append(value: V, now: number): Promise<void> {
    return this.chains.run("", () => this.appendNow(value, now));
  }

  private async appendNow(value: V, now: number): Promise<void> {
    const meta = await this.establishSkew();
    if (this.lastKeyTime === undefined) {
      const last = await this.overlay.list<unknown>({ prefix: logPrefix(this.field), reverse: true, limit: 1 });
      const [lastKey] = [...last.keys()];
      this.lastKeyTime = lastKey === undefined ? 0 : parseLogKey(lastKey).t;
    }
    // A key's time never goes backwards, so keys sort in append order; the entry keeps its own `t`.
    const keyTime = Math.max(this.lastKeyTime, now);
    meta.skew = Math.max(meta.skew as number, keyTime - now);
    this.overlay.put(logKey(this.field, keyTime, meta.seq), { t: now, v: this.codec.encode(value) });
    this.lastKeyTime = keyTime;
    meta.seq += 1;
    meta.size += 1;
    this.meta.touch();
  }

  prune(cutoff: number): Promise<void> {
    return this.chains.run("", () => this.pruneNow(cutoff));
  }

  private async pruneNow(cutoff: number): Promise<void> {
    const meta = await this.establishSkew();
    // An entry with `t < cutoff` has a key time below `cutoff + skew`.
    const stale = await listAll<StoredLogEntry>(this.overlay, { ...this.range(undefined, cutoff + (meta.skew as number)) });
    let removed = 0;
    for (const [k, e] of stale) {
      if (e.t < cutoff) {
        this.overlay.delete(k);
        removed += 1;
      }
    }
    if (removed > 0) {
      meta.size -= removed;
      this.meta.touch();
    }
  }

  async size(): Promise<number> {
    return (await this.meta.load()).size;
  }

  // The key range of key times `[from, to)`. Key times are never negative, so a bound below zero is zero.
  private range(from: number | undefined, to: number | undefined): DurableObjectListOptions {
    const clamp = (n: number | undefined): number | undefined => (n === undefined ? undefined : Math.max(n, 0));
    return logRange(this.field, clamp(from), clamp(to));
  }

  private async window(from: number | undefined, to: number | undefined, keep: (t: number) => boolean): Promise<LogEntry<V>[]> {
    const skew = (await this.meta.load()).skew;
    // The key time is never below an entry's `t` and at most `skew` above it; with no known skew an
    // upper bound cannot be trusted, so the window reads to the end of the log and filters.
    const upper = to === undefined || skew === undefined ? undefined : to + skew;
    const all = await listAll<StoredLogEntry>(this.overlay, this.range(from, upper));
    return all.filter(([, e]) => keep(e.t)).map(([, e]) => this.entry(e));
  }

  async entries(): Promise<ReadonlyArray<LogEntry<V>>> {
    return this.window(undefined, undefined, () => true);
  }

  async values(): Promise<readonly V[]> {
    return (await this.entries()).map((e) => e.v);
  }

  async since(t: number): Promise<readonly V[]> {
    return (await this.window(t, undefined, (x) => x >= t)).map((e) => e.v);
  }

  async before(t: number): Promise<readonly V[]> {
    return (await this.window(undefined, t, (x) => x < t)).map((e) => e.v);
  }

  async between(from: number, to: number): Promise<readonly V[]> {
    return (await this.window(from, to + 1, (x) => x >= from && x <= to)).map((e) => e.v);
  }

  async recent(n: number): Promise<readonly V[]> {
    const count = Math.max(0, n);
    if (count === 0) return [];
    const newest = await this.overlay.list<StoredLogEntry>({ prefix: logPrefix(this.field), reverse: true, limit: count });
    return [...newest.values()].map((e) => this.entry(e).v);
  }

  async reversed(): Promise<readonly V[]> {
    return [...(await this.values())].reverse();
  }

  finish(): void {
    this.meta.finish();
  }
}

/** How one collection field decodes and encodes its values, for the handle attached to it. */
export interface LazyFieldSpec {
  readonly name: string;
  readonly kind: "map" | "cache" | "set" | "log";
  /** Wire value to domain value; throws on a value that does not decode (a rehydration violation). */
  readonly decode?: (wire: unknown) => unknown;
  /** Domain value to wire value. */
  readonly encode?: (value: unknown) => unknown;
}

const identity = (v: unknown): unknown => v;

/**
 * Give a freshly merged state its storage-backed collection handles, in place of the in-memory
 * ones its zero carries (#3189, the lazy-read slice). The handles share one overlay, which the
 * commit flushes with the `"state"` record. `loadState` calls this instead of filling the
 * handles when `readStored` found a current-version per-entry record. An `@indexed` map keeps the
 * indexers its zero handle was built with.
 */
export function attachLazy(storage: DurableObjectStorage, state: object, specs: readonly LazyFieldSpec[]): void {
  const overlay = new EntryOverlay(storage);
  const slots = state as Record<string, unknown>;
  const handles: object[] = [];
  const finishers: Array<() => void> = [];
  const reapers: Array<(now: number, limit: number) => Promise<number>> = [];
  for (const spec of specs) {
    const decode = spec.decode ?? identity;
    const encode = spec.encode ?? identity;
    const was = slots[spec.name] as { indexers?: readonly Indexer<unknown>[] } | undefined;
    switch (spec.kind) {
      case "map": {
        const h = new LazyStoreMap<unknown>(
          overlay,
          spec.name,
          { read: (e: { v: unknown }) => decode(e.v), write: (v, i) => ({ i, v: encode(v) }) },
          was?.indexers ?? [],
        );
        slots[spec.name] = h;
        handles.push(h);
        finishers.push(() => h.finish());
        break;
      }
      case "cache": {
        const h = new LazyStoreCache<unknown>(overlay, spec.name, {
          read: (e: { v: unknown; exp: number }) => ({ v: decode(e.v), exp: e.exp }),
          write: (c, i) => ({ v: encode(c.v), exp: c.exp, i }),
          expiryOf: (e: { exp?: number }) => e.exp,
        });
        slots[spec.name] = h;
        handles.push(h);
        finishers.push(() => h.finish());
        reapers.push((now, limit) => h.reap(now, limit));
        break;
      }
      case "set": {
        const h = new LazyStoreSet(overlay, spec.name);
        slots[spec.name] = h;
        handles.push(h);
        finishers.push(() => h.finish());
        break;
      }
      case "log": {
        const h = new LazyStoreLog<unknown>(overlay, spec.name, { decode: decode as (w: never) => unknown, encode });
        slots[spec.name] = h;
        handles.push(h);
        finishers.push(() => h.finish());
        break;
      }
    }
  }
  setLazyContext(handles, { overlay, finish: () => finishers.forEach((f) => f()), reapers });
}
