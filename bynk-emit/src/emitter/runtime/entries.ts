import type { DurableObjectListOptions, DurableObjectStorage, DurableObjectTransaction } from "./storage.ts";
import { STORAGE_BATCH_LIMIT, utf8Compare } from "./storage.ts";
import { invariantViolation } from "./errors.ts";
import { __bynkKeyEncode } from "./keys.ts";
import { entriesOf, noteOrder } from "./store.ts";

// Per-entry agent storage: the runtime half (#3189, track #3195,
// `design/tracks/per-entry-storage.md`).
//
// Under the wholesale layout an agent's whole state is one value under the
// Durable Object key `"state"`. Under the per-entry layout the `"state"` record
// keeps only the `Cell` fields and the markers, and every `Map`/`Set`/`Cache`/
// `Log` entry and every `@indexed` posting lives under a key of its own. This
// module is the layout's vocabulary: the key shapes, a staged write-set that
// reads overlay-then-storage and flushes atomically, and the split of a
// wholesale record into per-entry writes.
//
// Nothing here is called by emitted code yet. The emitter wires it in with the
// layout change itself (a later slice of the track).

/** The reserved key in the `"state"` record naming how the record is laid out. */
export const LAYOUT_KEY = "__layout";
/** An absent `__layout`: every collection is inline in the `"state"` record. */
export const LAYOUT_WHOLESALE = 1;
/** Collection entries and index postings live under keys of their own. */
export const LAYOUT_PER_ENTRY = 2;

/** The layout a raw stored record is in, as far as its marker says. */
export function layoutOf(stored: object): number | undefined {
  const raw = (stored as { readonly __layout?: unknown }).__layout;
  if (raw === undefined) return LAYOUT_WHOLESALE;
  return typeof raw === "number" && Number.isInteger(raw) && raw >= 1 ? raw : undefined;
}

// A key part is escaped so that the separator can never appear inside one:
// `%` becomes `%25` and `/` becomes `%2F`. The escape is reversible, so a key
// listed back from storage decodes to the entry key it came from.
export function encodeKeyPart(part: string): string {
  return part.replace(/%/g, "%25").replace(/\//g, "%2F");
}

export function decodeKeyPart(part: string): string {
  return part.replace(/%2F/g, "/").replace(/%25/g, "%");
}

// Counters and timestamps in a key are zero-padded to a fixed width so that
// lexical (UTF-8 byte) order is numeric order. 16 digits hold any millisecond
// timestamp up to the year 316,000; 10 hold a per-field counter to 9,999,999,999.
const TIME_WIDTH = 16;
const SEQ_WIDTH = 10;

export function padNumber(n: number, width: number): string {
  if (!Number.isSafeInteger(n) || n < 0) {
    throw new RangeError(`a storage key number must be a non-negative safe integer, got ${String(n)}`);
  }
  const s = String(n);
  if (s.length > width) throw new RangeError(`a storage key number is wider than ${width} digits: ${s}`);
  return s.padStart(width, "0");
}

/** A `Map` or `Cache` entry. `k` is the entry key's existing string form. */
export const mapKey = (field: string, k: string): string => `m/${encodeKeyPart(field)}/${encodeKeyPart(k)}`;
export const mapPrefix = (field: string): string => `m/${encodeKeyPart(field)}/`;

/** A `Set` element (the value stored is `1`). */
export const setKey = (field: string, k: string): string => `s/${encodeKeyPart(field)}/${encodeKeyPart(k)}`;
export const setPrefix = (field: string): string => `s/${encodeKeyPart(field)}/`;

/** A `Log` entry: `t` is the append time in milliseconds and `seq` the field's counter. */
export const logKey = (field: string, t: number, seq: number): string =>
  `l/${encodeKeyPart(field)}/${padNumber(t, TIME_WIDTH)}-${padNumber(seq, SEQ_WIDTH)}`;
export const logPrefix = (field: string): string => `l/${encodeKeyPart(field)}/`;

/**
 * The `Log` entries with `t` in `[from, to)`, as `list` options: `from` and `to`
 * are timestamps, and the bounds sort before every entry at that millisecond.
 */
export function logRange(field: string, from: number | undefined, to: number | undefined): DurableObjectListOptions {
  const prefix = logPrefix(field);
  return {
    prefix,
    ...(from === undefined ? {} : { start: `${prefix}${padNumber(from, TIME_WIDTH)}-` }),
    ...(to === undefined ? {} : { end: `${prefix}${padNumber(to, TIME_WIDTH)}-` }),
  };
}

/** The key time and counter of a `Log` entry's storage key (`l/<field>/<t>-<seq>`). */
export function parseLogKey(key: string): { t: number; seq: number } {
  const tail = key.slice(key.lastIndexOf("/") + 1);
  const dash = tail.indexOf("-");
  return { t: Number(tail.slice(0, dash)), seq: Number(tail.slice(dash + 1)) };
}

/** One posting of a secondary index: `by` is the indexed value, `pk` the primary key. */
export function postingKey(field: string, index: string, by: string, pk: string): string {
  const key = `i/${encodeKeyPart(field)}/${encodeKeyPart(index)}/${encodeKeyPart(by)}/${encodeKeyPart(pk)}`;
  // The indexed value and the entry key sit in one storage key (#4037): fault here, naming the
  // field and the index, rather than at the commit flush with the platform's error.
  if (key.length * 3 > MAX_STORAGE_KEY_BYTES && new TextEncoder().encode(key).length > MAX_STORAGE_KEY_BYTES) {
    throw new Error(
      `store \`${field}\`: an index \`${index}\` key, its value and the entry's key together, is over the ${MAX_STORAGE_KEY_BYTES}-byte storage-key limit`,
    );
  }
  return key;
}
/** Every posting of every index of a map field. */
export const postingFieldPrefix = (field: string): string => `i/${encodeKeyPart(field)}/`;
/** Every posting of one indexed value; with `by` omitted, every posting of the index. */
export const postingPrefix = (field: string, index: string, by?: string): string =>
  `i/${encodeKeyPart(field)}/${encodeKeyPart(index)}/${by === undefined ? "" : `${encodeKeyPart(by)}/`}`;

/** A `Cache` expiry entry, ordered by expiry time so reaping is a bounded range read. */
export const expiryKey = (field: string, exp: number, k: string): string =>
  `x/${encodeKeyPart(field)}/${padNumber(exp, TIME_WIDTH)}/${encodeKeyPart(k)}`;
export const expiryPrefix = (field: string): string => `x/${encodeKeyPart(field)}/`;
/** The expiry time and entry key an expiry key (`x/<field>/<exp>/<key>`) carries. */
export function parseExpiryKey(field: string, storageKey: string): { exp: number; key: string } {
  const rest = storageKey.slice(expiryPrefix(field).length);
  return { exp: Number(rest.slice(0, TIME_WIDTH)), key: decodeKeyPart(rest.slice(TIME_WIDTH + 1)) };
}
/** The expiry entries with `exp <= now`, as `list` options. */
export function expiredRange(field: string, now: number): DurableObjectListOptions {
  const prefix = expiryPrefix(field);
  return { prefix, end: `${prefix}${padNumber(now + 1, TIME_WIDTH)}/` };
}

/** Per-collection metadata: the entry count for `size()` and a `Log`'s counter. */
export const metaKey = (field: string): string => `c/${encodeKeyPart(field)}`;

/** The longest a Durable Object storage key may be, in bytes. */
export const MAX_STORAGE_KEY_BYTES = 2048;

/**
 * The entry key of a store collection's non-scalar key (#4037): its canonical encoding, the one
 * a value `Map` indexes by, so two keys are one entry exactly when they are `==`. The encoding
 * counts towards the platform's key size, and the longest key it appears in is a `Cache`'s expiry
 * entry, so a key over the limit faults here, naming the field and never the key's value.
 */
export function storeKey(field: string, key: unknown): string {
  const enc = __bynkKeyEncode(key);
  if (new TextEncoder().encode(expiryKey(field, 0, enc)).length > MAX_STORAGE_KEY_BYTES) {
    throw new Error(`store \`${field}\`: a key's encoding is over the ${MAX_STORAGE_KEY_BYTES}-byte storage-key limit`);
  }
  return enc;
}


/**
 * The committed entry count of a collection field, for `old.<field>.size()` in a
 * `transition`: the stored count, or for a record still in the wholesale layout
 * the entries its inline collection holds. A collection with neither is empty.
 */
export async function committedSize(
  storage: DurableObjectStorage,
  field: string,
  prior: object,
): Promise<number> {
  const meta = await storage.get<{ size?: number }>(metaKey(field));
  if (meta !== undefined) return meta.size ?? 0;
  const inline = (prior as Readonly<Record<string, unknown>>)[field];
  return inline === undefined || inline === null ? 0 : Object.keys(inline as object).length;
}

/** The entry key a listed storage key carries, given the prefix it was listed under. */
export function entryKeyOf(prefix: string, storageKey: string): string {
  return decodeKeyPart(storageKey.slice(prefix.length));
}

/**
 * A handler's staged writes over one agent's storage. Reads see the handler's own
 * writes (read-your-writes, ADR 0109 D2): a key written here reads back its
 * staged value, a key deleted here reads as absent, and anything else is read
 * from storage. Nothing reaches storage until {@link EntryOverlay.flush}, which
 * writes every staged put and delete in one transaction, so a handler that
 * faults before the flush persists nothing.
 */
export class EntryOverlay {
  private readonly storage: DurableObjectStorage;
  private puts = new Map<string, unknown>();
  private deletes = new Set<string>();

  constructor(storage: DurableObjectStorage) {
    this.storage = storage;
  }

  /** The number of staged writes (puts and deletes). */
  get pending(): number {
    return this.puts.size + this.deletes.size;
  }

  put(key: string, value: unknown): void {
    if (value === undefined) throw new TypeError(`cannot stage an undefined value for storage key ${key}`);
    this.deletes.delete(key);
    this.puts.set(key, value);
  }

  delete(key: string): void {
    this.puts.delete(key);
    this.deletes.add(key);
  }

  async get<T>(key: string): Promise<T | undefined> {
    if (this.puts.has(key)) return this.puts.get(key) as T;
    if (this.deletes.has(key)) return undefined;
    return this.storage.get<T>(key);
  }

  /** Several keys at once: staged values first, the rest in batched reads. */
  async getMany<T>(keys: readonly string[]): Promise<Map<string, T>> {
    const out = new Map<string, T>();
    const missing: string[] = [];
    for (const k of new Set(keys)) {
      if (this.puts.has(k)) out.set(k, this.puts.get(k) as T);
      else if (!this.deletes.has(k)) missing.push(k);
    }
    for (let i = 0; i < missing.length; i += STORAGE_BATCH_LIMIT) {
      const found = await this.storage.get<T>(missing.slice(i, i + STORAGE_BATCH_LIMIT));
      for (const [k, v] of found) out.set(k, v);
    }
    return new Map([...out].sort(([a], [b]) => utf8Compare(a, b)));
  }

  /**
   * A range of keys as the handler sees them: storage's entries, minus the keys it
   * deleted, plus the keys it put, in key order and within `limit`. Storage is read
   * with only as many extra entries as the handler's deletes in range could remove,
   * so a bounded read stays bounded.
   */
  async list<T>(options: DurableObjectListOptions = {}): Promise<Map<string, T>> {
    const inRange = (k: string): boolean => {
      if (options.prefix !== undefined && !k.startsWith(options.prefix)) return false;
      if (options.start !== undefined && utf8Compare(k, options.start) < 0) return false;
      if (options.startAfter !== undefined && utf8Compare(k, options.startAfter) <= 0) return false;
      if (options.end !== undefined && utf8Compare(k, options.end) >= 0) return false;
      return true;
    };
    let removed = 0;
    for (const k of this.deletes) if (inRange(k)) removed += 1;
    const fromStorage = await this.storage.list<T>({
      ...options,
      ...(options.limit === undefined ? {} : { limit: options.limit + removed }),
    });
    const merged = new Map<string, T>();
    for (const [k, v] of fromStorage) if (!this.deletes.has(k)) merged.set(k, v);
    for (const [k, v] of this.puts) if (inRange(k)) merged.set(k, v as T);
    const keys = [...merged.keys()].sort(utf8Compare);
    if (options.reverse === true) keys.reverse();
    const limited = options.limit === undefined ? keys : keys.slice(0, options.limit);
    return new Map(limited.map((k) => [k, merged.get(k) as T]));
  }

  /**
   * Write every staged put and delete, and `extra` (typically the `"state"` record),
   * in one transaction, in batches within the platform's limit, then clear the
   * overlay. A throw anywhere rolls the whole flush back. On a throw the overlay is
   * left as it was. A write staged while the flush is awaiting is kept for the next
   * flush, not dropped: only what this flush wrote is cleared.
   */
  async flush(extra: Readonly<Record<string, unknown>> = {}): Promise<void> {
    const puts: Array<[string, unknown]> = [...this.puts, ...Object.entries(extra)];
    const deletes = [...this.deletes];
    if (puts.length === 0 && deletes.length === 0) return;
    await this.storage.transaction(async (txn: DurableObjectTransaction) => {
      for (let i = 0; i < deletes.length; i += STORAGE_BATCH_LIMIT) {
        await txn.delete(deletes.slice(i, i + STORAGE_BATCH_LIMIT));
      }
      for (let i = 0; i < puts.length; i += STORAGE_BATCH_LIMIT) {
        await txn.put(Object.fromEntries(puts.slice(i, i + STORAGE_BATCH_LIMIT)));
      }
    });
    // Clear only what was written, and only where it has not been restaged since.
    for (const [k, v] of puts) if (this.puts.get(k) === v) this.puts.delete(k);
    for (const k of deletes) if (this.deletes.has(k)) this.deletes.delete(k);
  }

  /** Drop every staged write, as a faulted handler does. */
  discard(): void {
    this.puts = new Map();
    this.deletes = new Set();
  }
}

/**
 * Move an indexed entry's posting when the value it is indexed under changes: the
 * old posting is deleted and the new one put, both staged in the one write-set.
 * `oldBy` is `undefined` for a new entry and `newBy` for a removed one.
 */
export function reindex(
  overlay: EntryOverlay,
  field: string,
  index: string,
  pk: string,
  oldBy: string | undefined,
  newBy: string | undefined,
): void {
  if (oldBy === newBy) return;
  if (oldBy !== undefined) overlay.delete(postingKey(field, index, oldBy, pk));
  if (newBy !== undefined) overlay.put(postingKey(field, index, newBy, pk), 1);
}

/** How one collection field is held in a wholesale `"state"` record. */
export type WholesaleField =
  | { readonly name: string; readonly kind: "map" | "cache"; readonly indexes?: readonly WholesaleIndex[] }
  | { readonly name: string; readonly kind: "set"; readonly indexes?: undefined }
  | { readonly name: string; readonly kind: "log"; readonly indexes?: undefined };

/**
 * An `@indexed` field's posting record in a wholesale `"state"` record: a
 * `Record<by, primaryKey[]>` held under `recordField`.
 */
export interface WholesaleIndex {
  readonly index: string;
  readonly recordField: string;
  /** A `@unique` index (#4038): its postings key an `Option` by its payload, not the whole option. */
  readonly unique?: boolean;
}

/**
 * The index set a map's metadata records (#4038): each index and its key scheme, sorted. A handle
 * whose declared set differs from the recorded one rebuilds the field's postings before writing.
 */
export function indexSignature(indexes: ReadonlyArray<{ readonly index: string; readonly unique?: boolean }>): string {
  return indexes
    .map((ix) => `${ix.index}:${ix.unique === true ? "u" : "i"}`)
    .sort()
    .join(",");
}

/** A map's metadata record: its size and sequence, and its index set when it has one. */
const mapMeta = (size: number, seq: number | undefined, indexes: ReadonlyArray<WholesaleIndex> | undefined): Record<string, unknown> => {
  const ix = indexSignature(indexes ?? []);
  return { size, ...(seq === undefined ? {} : { seq }), ...(ix === "" ? {} : { ix }) };
};

/** What splitting a wholesale record produces. */
export interface SplitRecord {
  /** The `"state"` record with every collection and posting field removed and the layout marker set. */
  readonly record: Record<string, unknown>;
  /** Every per-entry key and its value. */
  readonly entries: Record<string, unknown>;
}

/**
 * Split a wholesale `"state"` record into the trimmed record and the per-entry
 * writes (the lazy upgrade, #3189). The input is not changed. Everything that is
 * not a named collection or posting field, `__v` included, stays in the record.
 * A `Log` entry takes its stored append time and a counter that follows the array
 * order, so entries appended in one millisecond keep their order.
 */
export function splitWholesale(stored: Readonly<Record<string, unknown>>, fields: readonly WholesaleField[]): SplitRecord {
  const record: Record<string, unknown> = { ...stored };
  const entries: Record<string, unknown> = {};
  for (const f of fields) {
    const inline = record[f.name];
    delete record[f.name];
    // A posting record never outlives its map, so it leaves the record either way.
    const postingRecords = (f.indexes ?? []).map((ix) => {
      const postings = record[ix.recordField];
      delete record[ix.recordField];
      return { ix, postings };
    });
    if (inline === undefined || inline === null) continue;
    switch (f.kind) {
      case "map":
      case "cache": {
        for (const [k, v] of Object.entries(inline as Record<string, unknown>)) {
          entries[mapKey(f.name, k)] = v;
          if (f.kind === "cache") {
            const exp = (v as { exp?: unknown }).exp;
            if (typeof exp === "number") entries[expiryKey(f.name, exp, k)] = 1;
          }
        }
        for (const { ix, postings } of postingRecords) {
          if (postings === undefined || postings === null) continue;
          for (const [by, pks] of Object.entries(postings as Record<string, readonly string[]>)) {
            for (const pk of pks) entries[postingKey(f.name, ix.index, by, pk)] = 1;
          }
        }
        entries[metaKey(f.name)] = mapMeta(Object.keys(inline as object).length, undefined, f.indexes);
        break;
      }
      case "set": {
        const members = Object.keys(inline as Record<string, unknown>);
        for (const k of members) entries[setKey(f.name, k)] = 1;
        entries[metaKey(f.name)] = { size: members.length };
        break;
      }
      case "log": {
        const log = inline as ReadonlyArray<{ t: number }>;
        log.forEach((e, seq) => {
          entries[logKey(f.name, e.t, seq)] = e;
        });
        entries[metaKey(f.name)] = { size: log.length, seq: log.length };
        break;
      }
    }
  }
  record[LAYOUT_KEY] = LAYOUT_PER_ENTRY;
  return { record, entries };
}

// Structural equality of two stored (wire-shape, JSON-like) values: plain data only.
export function sameStored(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (typeof a !== "object" || typeof b !== "object" || a === null || b === null) return false;
  if (Array.isArray(a) !== Array.isArray(b)) return false;
  if (Array.isArray(a)) {
    const y = b as readonly unknown[];
    return a.length === y.length && a.every((v, i) => sameStored(v, y[i]));
  }
  const ka = Object.keys(a as object);
  const kb = Object.keys(b as object);
  if (ka.length !== kb.length) return false;
  return ka.every((k) => Object.hasOwn(b as object, k) && sameStored((a as Record<string, unknown>)[k], (b as Record<string, unknown>)[k]));
}

/** What a commit has to write to take storage from `base` to `next`. */
export interface EntryDiff {
  /** Keys whose value is new or changed, with the new value. */
  readonly puts: Record<string, unknown>;
  /** Keys present in `base` and absent from `next`. */
  readonly deletes: readonly string[];
}

/**
 * The writes that take the per-entry keys a handler read (`base`) to the keys its
 * final state encodes to (`next`): only what changed. An unchanged entry is not
 * written, so a handler that touches one entry of a large collection writes one key.
 */
export function diffEntries(base: ReadonlyMap<string, unknown>, next: Readonly<Record<string, unknown>>): EntryDiff {
  const puts: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(next)) {
    if (!base.has(k) || !sameStored(base.get(k), v)) puts[k] = v;
  }
  const deletes: string[] = [];
  for (const k of base.keys()) if (!Object.hasOwn(next, k)) deletes.push(k);
  return { puts, deletes };
}

// ─────────────────────────────────────────────────────────────────────────────
// The per-entry layout as a codec around the stored record (#3189, slice 2).
//
// An agent's state is encoded and decoded as one wholesale-shaped record (each
// `Map`/`Set`/`Cache` a `Record`, each `Log` an array, each index a `Record` of
// posting lists), whatever the layout it is stored in. `readStored` turns what is
// stored into that record; `commitStored` writes one back, per entry. The emitted
// agent only passes a description of its collection fields.
//
// Frozen layout (layout 2; the stored bytes a later, lazy-reading slice must not
// change):
//  - `"state"`: the `Cell` fields, `__v` and `__layout: 2`. No collection is in it.
//  - `m/<field>/<k>`: a `Map` entry `{ i, v }`; a `Cache` entry `{ v, exp, i }`.
//  - `s/<field>/<k>`: a `Set` element `{ i }`.
//  - `l/<field>/<t>-<seq>`: a `Log` entry `{ t, v }`.
//  - `i/<field>/<index>/<by>/<pk>`: one index posting, value `1`.
//  - `x/<field>/<exp>/<k>`: a `Cache` expiry entry, value `1`.
//  - `c/<field>`: `{ size, seq }`: the entry count, and the next insertion sequence
//    (a `Log`'s next `<seq>`).
// `i` is the entry's insertion sequence: a new key takes the field's next `seq`, an
// overwrite keeps its `i`, a key removed and inserted again takes a new one. Order
// is `i`, so insertion order survives the keys' own byte order. A posting list's
// order is its entries' `i` order.

/** What a collection field holds, as the emitter describes it. */
export type EntryField = WholesaleField;

/** Every per-entry key an agent read, as stored, and each field's metadata. */
interface EntryBase {
  entries: Map<string, unknown>;
}

const bases = new WeakMap<object, EntryBase>();

/** What a lazy load staged for the commit: the shared overlay, each handle's metadata, each `Cache`'s reaper. */
export interface LazyContext {
  readonly overlay: EntryOverlay;
  finish(): void;
  readonly reapers: ReadonlyArray<(now: number, limit: number) => Promise<number>>;
}

// Keyed by the storage-backed handles of the state a handler loaded, not by the agent instance: the
// commit finds the overlay of the handler whose state it is committing, whatever else has loaded since.
const lazyContexts = new WeakMap<object, LazyContext>();
// Instances whose last `readStored` chose the lazy read: `loadState` then attaches handles.
const lazyReads = new WeakMap<object, true>();

/** Register the context every one of `handles` belongs to. */
export function setLazyContext(handles: readonly object[], ctx: LazyContext): void {
  for (const h of handles) lazyContexts.set(h, ctx);
}

// The context of the lazy handles in `state`'s collection slots, if it holds any.
function lazyContextOf(state: object | undefined, fields: readonly EntryField[]): LazyContext | undefined {
  if (state === undefined) return undefined;
  for (const f of fields) {
    const handle = (state as Record<string, unknown>)[f.name];
    const ctx = typeof handle === "object" && handle !== null ? lazyContexts.get(handle) : undefined;
    if (ctx !== undefined) return ctx;
  }
  return undefined;
}

/** Whether `self`'s last `readStored` took the lazy read (its collections are not in the record it returned). */
export function readsLazily(self: object): boolean {
  return lazyReads.has(self);
}

/**
 * Whether a stored record is read lazily: a current-version per-entry record. A record in the
 * wholesale layout, or one a migration step still covers (`steps` are the `[lo, from]` version
 * ranges, as `__checkStoredSchemaVersion` takes them), is read whole.
 */
export function canReadLazily(stored: Readonly<Record<string, unknown>>, steps: ReadonlyArray<readonly [number, number]>): boolean {
  if ((stored as { __layout?: unknown }).__layout !== LAYOUT_PER_ENTRY) return false;
  const v = typeof stored["__v"] === "number" ? (stored["__v"] as number) : 1;
  return !steps.some(([lo, from]) => v >= lo && v <= from);
}

function copyStored<T>(v: T): T {
  return globalThis.structuredClone(v);
}

const ENTRY_PREFIXES = ["m/", "s/", "l/", "i/", "x/", "c/"];

function isEntryKey(k: string): boolean {
  return ENTRY_PREFIXES.some((p) => k.startsWith(p));
}

interface Seq {
  readonly i: number;
}
const orderOf = (v: unknown): number => (v as Seq).i;

/**
 * The agent's stored state as a wholesale-shaped record, whatever layout it is in.
 * A record in the per-entry layout has its entries read and joined back, in
 * insertion order. A wholesale record is returned as it is and not written: its
 * first `commitStored` splits it, in one transaction. `self` keys the base the next
 * `commitStored` diffs against.
 */
export async function readStored<S extends object>(
  self: object,
  storage: DurableObjectStorage,
  storedState: S,
  lazySteps?: ReadonlyArray<readonly [number, number]>,
): Promise<S> {
  const stored = storedState as Readonly<Record<string, unknown>>;
  lazyReads.delete(self);
  if (lazySteps !== undefined && canReadLazily(stored, lazySteps)) {
    // Lazy read: the Cell fields are the whole record; the collections are read by their handles.
    bases.delete(self);
    lazyReads.set(self, true);
    const out: Record<string, unknown> = { ...stored };
    delete out[LAYOUT_KEY];
    return out as S;
  }
  if ((stored as { __layout?: unknown }).__layout === undefined) {
    // The wholesale layout. Nothing is written on load: the record is handed back as it
    // is, for any migration step to read, and the first commit splits the migrated state
    // (diffed against an empty base, so every entry is written) with the marker in one
    // transaction. A handler that only reads, or faults, leaves the record untouched.
    bases.set(self, { entries: new Map() });
    return { ...stored } as S;
  }
  const all = await storage.list<unknown>();
  const entries = new Map<string, unknown>();
  for (const [k, v] of all) if (isEntryKey(k)) entries.set(k, v);
  bases.set(self, { entries });
  return joinStored(stored, entries) as S;
}

// ─────────────────────────────────────────────────────────────────────────────
// A collection `store` field renamed with `replaces` (#4482).
//
// A `Map` or `Set` keeps each entry under a key that carries the field's name, so a rename moves
// keys; reading the old name is not enough. `moveRenamedEntries` moves them in bounded batches, each
// a transaction that writes the entries under the new name and deletes them from the old one, so
// an entry is always under exactly one name and a crash between batches leaves a state the next
// load continues from. An entry already under the new name wins and the old one is dropped. The
// field's metadata (`c/`) moves last, and a marker (`r/<new>`, which a commit never rewrites)
// records that the move is done, so later loads skip the listing.

/** One collection rename: `from` is the field's earlier name, `to` its name now. `kind` is its key tag. */
export interface CollectionRename {
  readonly kind: "m" | "s";
  readonly from: string;
  readonly to: string;
}

/** How many entries one batch moves: Cloudflare's limit on keys per batched write. */
export const RENAME_BATCH = 128;

const renameMarkerKey = (field: string): string => `r/${encodeKeyPart(field)}`;

// Instances whose renames this isolate has finished, so only the first load pays for the check.
const renamesDone = new WeakMap<object, true>();

/**
 * Move every entry of each renamed collection from under its old name to its new one. Called from a
 * load, which an instance runs one handler at a time, before anything reads the collection. The
 * first load after a deploy does the whole move, and its cost grows with the collection.
 */
export async function moveRenamedEntries(
  self: object,
  storage: DurableObjectStorage,
  renames: readonly CollectionRename[],
  batch: number = RENAME_BATCH,
): Promise<void> {
  if (renamesDone.has(self)) return;
  for (const rename of renames) await moveOneRename(storage, rename, batch);
  renamesDone.set(self, true);
}

async function moveOneRename(storage: DurableObjectStorage, r: CollectionRename, batch: number): Promise<void> {
  const marker = renameMarkerKey(r.to);
  const done = await storage.get<{ from?: string[] }>(marker);
  if (done?.from?.includes(r.from)) return;
  const oldPrefix = `${r.kind}/${encodeKeyPart(r.from)}/`;
  const newPrefix = `${r.kind}/${encodeKeyPart(r.to)}/`;
  for (;;) {
    const page = await storage.list<unknown>({ prefix: oldPrefix, limit: batch });
    if (page.size === 0) break;
    const oldKeys = [...page.keys()];
    const newKeys = oldKeys.map((k) => newPrefix + k.slice(oldPrefix.length));
    const present = await storage.get<unknown>(newKeys);
    const moved: Record<string, unknown> = {};
    oldKeys.forEach((k, at) => {
      if (!present.has(newKeys[at]!)) moved[newKeys[at]!] = page.get(k);
    });
    await storage.transaction(async (txn) => {
      await txn.put(moved);
      await txn.delete(oldKeys);
    });
  }
  // The metadata (the entry count and the next insertion sequence) moves with the entries; a
  // metadata record already under the new name is the field's own.
  const oldMeta = await storage.get<unknown>(metaKey(r.from));
  const newMeta = await storage.get<unknown>(metaKey(r.to));
  await storage.transaction(async (txn) => {
    if (oldMeta !== undefined) {
      if (newMeta === undefined) await txn.put(metaKey(r.to), oldMeta);
      await txn.delete(metaKey(r.from));
    }
    await txn.put(marker, { from: [...(done?.from ?? []), r.from] });
  });
}

// Join every per-entry key found in storage back into a wholesale-shaped record. It is
// driven by what is stored, not by the current class's field list: a migration step runs
// on the record as the *stored* version shaped it, so a collection the current version
// removed, or re-kinded, must be joined as it was stored. A collection field is rebuilt
// from the prefix its entries are under (`m/` a Map, or a Cache when its entries carry an
// expiry; `s/` a Set; `l/` a Log). An index posting is not joined: a `Map` handle rebuilds
// its postings from the entries it is filled with. A field that has an inline value in the
// record and no entries is left as it is.
function joinStored(
  stored: Readonly<Record<string, unknown>>,
  entries: ReadonlyMap<string, unknown>,
): Record<string, unknown> {
  const out: Record<string, unknown> = { ...stored };
  delete out[LAYOUT_KEY];
  const maps = new Map<string, Array<[string, Record<string, unknown>]>>();
  const sets = new Map<string, Array<[string, Record<string, unknown>]>>();
  const logs = new Map<string, Array<[string, unknown]>>();
  const split = (key: string, parts: number): string[] => {
    const all = key.slice(2).split("/");
    return all.length === parts ? all.map(decodeKeyPart) : [];
  };
  for (const [key, value] of entries) {
    const tag = key.slice(0, 2);
    if (tag === "m/" || tag === "s/") {
      const [field, k] = split(key, 2);
      if (field === undefined || k === undefined) continue;
      const into = tag === "m/" ? maps : sets;
      into.set(field, [...(into.get(field) ?? []), [k, value as Record<string, unknown>]]);
    } else if (tag === "l/") {
      const [field] = split(key, 2);
      if (field === undefined) continue;
      logs.set(field, [...(logs.get(field) ?? []), [key, value]]);
    }
  }
  const define = (name: string, value: unknown): void => {
    Object.defineProperty(out, name, { value, enumerable: true, writable: true, configurable: true });
  };
  const record = (
    own: Array<[string, Record<string, unknown>]>,
    read: (env: Record<string, unknown>) => unknown,
  ): Record<string, unknown> => {
    own.sort(([, a], [, b]) => orderOf(a) - orderOf(b));
    const value: Record<string, unknown> = {};
    for (const [k, env] of own) {
      Object.defineProperty(value, k, { value: read(env), enumerable: true, writable: true, configurable: true });
    }
    // A record enumerates integer-like keys first, whatever order they went in: note the
    // insertion order beside it for the handle that is filled from it.
    noteOrder(
      value,
      own.map(([k]) => k),
    );
    return value;
  };
  for (const [field, own] of maps) {
    const cache = own.every(([, env]) => Object.hasOwn(env, "exp"));
    define(
      field,
      record(own, (env) => {
        if (!cache) return copyStored(env.v);
        const { i: _i, ...rest } = env;
        return copyStored(rest);
      }),
    );
  }
  for (const [field, own] of sets) {
    define(
      field,
      record(own, () => true),
    );
  }
  for (const [field, own] of logs) {
    own.sort(([a], [b]) => utf8Compare(a, b));
    define(
      field,
      own.map(([, v]) => copyStored(v)),
    );
  }
  return out;
}

interface Planned {
  /** The `"state"` record to write. */
  record: Record<string, unknown>;
  /** Every per-entry key the state now has. */
  entries: Record<string, unknown>;
}

// Plan the per-entry form of an encoded (wholesale-shaped) state record, giving each
// entry its insertion sequence against `base`.
function planEntries(
  encoded: Readonly<Record<string, unknown>>,
  fields: readonly EntryField[],
  base: ReadonlyMap<string, unknown>,
  now: number | undefined,
): Planned {
  const record: Record<string, unknown> = { ...encoded };
  const entries: Record<string, unknown> = {};
  let reapBudget = CACHE_REAP_LIMIT;
  for (const f of fields) {
    const inline = record[f.name];
    delete record[f.name];
    const postingRecords = (f.indexes ?? []).map((ix) => {
      const postings = record[ix.recordField];
      delete record[ix.recordField];
      return { ix, postings };
    });
    const meta = base.get(metaKey(f.name)) as { size?: number; seq?: number; skew?: number } | undefined;
    let seq = meta?.seq ?? 0;
    if (inline === undefined || inline === null) {
      continue;
    }
    if (f.kind === "log") {
      const log = inline as ReadonlyArray<{ t: number }>;
      const own = [...base].filter(([k]) => k.startsWith(logPrefix(f.name))).sort(([a], [b]) => utf8Compare(a, b));
      // A log only appends and prunes its oldest: the new array is a suffix of the stored
      // one followed by new entries. Find how many were pruned; if it is not that, rewrite.
      let pruned = -1;
      for (let p = 0; p <= own.length; p += 1) {
        const kept = own.length - p;
        if (kept <= log.length && own.slice(p).every(([, v], j) => sameStored(v, log[j]))) {
          pruned = p;
          break;
        }
      }
      if (pruned < 0) pruned = own.length; // nothing kept: every old key goes, every entry is new
      const kept = own.length - pruned;
      own.slice(pruned).forEach(([k, v]) => {
        entries[k] = v;
      });
      // A key's time never goes backwards: an entry appended with an earlier `t` than the
      // last (the clock stepped back, or isolates disagree) takes the last key's time, so
      // keys sort in append order. The entry keeps its own `t`.
      const last = own.length > 0 ? own[own.length - 1]![0] : undefined;
      let keyTime = last === undefined ? 0 : Number(last.slice(logPrefix(f.name).length, logPrefix(f.name).length + TIME_WIDTH));
      // The most a key's time is above its entry's `t`: a window read widens by it (`skew`). It is
      // exact over every entry the log now holds, so it is always recorded (a record without it
      // means "unknown", never zero).
      let skew = 0;
      own.slice(pruned).forEach(([k, v]) => {
        skew = Math.max(skew, Number(k.slice(logPrefix(f.name).length, logPrefix(f.name).length + TIME_WIDTH)) - (v as { t: number }).t);
      });
      log.slice(kept).forEach((e) => {
        keyTime = Math.max(keyTime, e.t);
        skew = Math.max(skew, keyTime - e.t);
        entries[logKey(f.name, keyTime, seq)] = e;
        seq += 1;
      });
      if (log.length > 0 || meta !== undefined) entries[metaKey(f.name)] = { size: log.length, seq, skew };
      continue;
    }
    const keyOf = f.kind === "set" ? setKey : mapKey;
    let maxSeen = -1;
    // The collection in insertion order (the wire record notes it; see `entriesOf`).
    let collection = entriesOf(inline as Record<string, unknown>);
    // Cache reaping (#3189): an entry the program can no longer see (its `exp` is not after the
    // clock the program last read) is not written back, so its key and its expiry entry are
    // deleted by the diff. Bounded per commit, oldest expiry first.
    if (f.kind === "cache" && now !== undefined && reapBudget > 0) {
      const expired = collection
        .filter(([, c]) => typeof (c as { exp?: unknown }).exp === "number" && (c as { exp: number }).exp <= now)
        .sort((a, b) => (a[1] as { exp: number }).exp - (b[1] as { exp: number }).exp)
        .slice(0, reapBudget);
      if (expired.length > 0) {
        reapBudget -= expired.length;
        const gone = new Set(expired.map(([k]) => k));
        collection = collection.filter(([k]) => !gone.has(k));
      }
    }
    for (const [k, v] of collection) {
      const old = base.get(keyOf(f.name, k)) as Seq | undefined;
      let i: number;
      if (old === undefined || old.i < maxSeen) {
        // A new key, or one inserted again after later keys (it moved to the end).
        i = seq;
        seq += 1;
      } else {
        i = old.i;
      }
      maxSeen = Math.max(maxSeen, i);
      if (f.kind === "set") entries[keyOf(f.name, k)] = { i };
      else if (f.kind === "map") entries[keyOf(f.name, k)] = { i, v };
      else {
        const c = v as { v: unknown; exp: unknown };
        entries[keyOf(f.name, k)] = { v: c.v, exp: c.exp, i };
        if (typeof c.exp === "number") entries[expiryKey(f.name, c.exp, k)] = 1;
      }
    }
    for (const { ix, postings } of postingRecords) {
      if (postings === undefined || postings === null) continue;
      for (const [by, pks] of Object.entries(postings as Record<string, readonly string[]>)) {
        for (const pk of pks) entries[postingKey(f.name, ix.index, by, pk)] = 1;
      }
    }
    const size = collection.length;
    if (size > 0 || meta !== undefined) entries[metaKey(f.name)] = f.kind === "map" ? mapMeta(size, seq, f.indexes) : { size, seq };
  }
  record[LAYOUT_KEY] = LAYOUT_PER_ENTRY;
  return { record, entries };
}

async function writeEntries(
  self: object,
  storage: DurableObjectStorage,
  encoded: Readonly<Record<string, unknown>>,
  fields: readonly EntryField[],
  now: number | undefined,
): Promise<void> {
  const base = bases.get(self)?.entries ?? new Map<string, unknown>();
  const { record, entries } = planEntries(encoded, fields, base, now);
  const { puts, deletes } = diffEntries(base, entries);
  const overlay = new EntryOverlay(storage);
  for (const [k, v] of Object.entries(puts)) overlay.put(k, v);
  for (const k of deletes) overlay.delete(k);
  await overlay.flush({ state: record });
  // The base keeps its own copies: a handler mutates the values it was given in place, and
  // that must show up as a difference from the base, not change it too.
  const next = new Map(base);
  for (const k of deletes) next.delete(k);
  for (const [k, v] of Object.entries(puts)) next.set(k, copyStored(v));
  bases.set(self, { entries: next });
}

// A lazy handler's commit: reap expired `Cache` entries (bounded, as the eager commit does), stage
// each handle's metadata, and write the `"state"` record with the writes the handles staged, in
// one transaction. The collections' own writes are already in the overlay; the encoded record's
// collection slots are empty placeholders and leave it.
async function commitLazy(
  ctx: LazyContext,
  encoded: Readonly<Record<string, unknown>>,
  fields: readonly EntryField[],
  now: number | undefined,
): Promise<void> {
  if (now !== undefined) {
    let budget = CACHE_REAP_LIMIT;
    for (const reap of ctx.reapers) {
      if (budget <= 0) break;
      budget -= await reap(now, budget);
    }
  }
  ctx.finish();
  const record: Record<string, unknown> = { ...encoded };
  for (const f of fields) {
    delete record[f.name];
    for (const ix of f.indexes ?? []) delete record[ix.recordField];
  }
  record[LAYOUT_KEY] = LAYOUT_PER_ENTRY;
  await ctx.overlay.flush({ state: record });
}

/**
 * Write an encoded (wholesale-shaped) state record in the per-entry layout: the `"state"`
 * record, and only the entries that changed since `readStored`, in one transaction. The
 * base is replaced only once the write has landed, so a write that throws leaves both
 * storage and the base as they were.
 */
export async function commitStored(
  self: object,
  storage: DurableObjectStorage,
  encoded: object,
  fields: readonly EntryField[],
  state?: object,
): Promise<void> {
  const now = state === undefined ? undefined : clocks.get(state);
  const lazy = lazyContextOf(state, fields);
  if (lazy !== undefined) {
    await commitLazy(lazy, encoded as Readonly<Record<string, unknown>>, fields, now);
    return;
  }
  await writeEntries(self, storage, encoded as Readonly<Record<string, unknown>>, fields, now);
}

// Cache reaping (#3189). A `Cache` operation reads the program's injected `Clock`, never the
// runtime's own: under a fake clock the real time is later than every stamped `exp`, so a
// runtime-clock reaper would delete entries the program still sees as live. The operation
// notes what it read against the state object the handler commits; the commit reaps with it.
// A handler that never touched a `Cache` notes nothing, and its commit reaps nothing.
const clocks = new WeakMap<object, number>();

/**
 * #3931: `self.erase()`. A handler that calls it carries an `Erasure`, marked by the call, and its commit
 * deletes the instance's storage instead of writing the handler's state.
 */
export interface Erasure {
  erased: boolean;
}

export function newErasure(): Erasure {
  return { erased: false };
}

/** The erasure `self.erase()` marks: an effect, so `do self.erase()` and a tail `self.erase()` read as any other. */
export function markErased(erasure: Erasure): Promise<void> {
  erasure.erased = true;
  return Promise.resolve();
}

/**
 * The commit of a handler that erased: delete every key of the instance's storage (the `"state"` record, the
 * layout and schema markers, each per-entry key and index posting), and forget what was read of it, so the next
 * call finds a key it has never seen. Nothing of the handler's own writes is kept, and no invariant is checked
 * against the initial state the instance returns to.
 */
export async function eraseStored(self: object, storage: DurableObjectStorage): Promise<void> {
  await storage.deleteAll();
  bases.delete(self);
  lazyReads.delete(self);
}

/** The most expired entries one commit deletes, across all of an agent's `Cache` fields. */
export const CACHE_REAP_LIMIT = 64;

/** Note the clock a `Cache` operation read, and return it (so it wraps the read in place). */
export function noteClock(state: object, now: number): number {
  const seen = clocks.get(state);
  if (seen === undefined || now > seen) clocks.set(state, now);
  return now;
}

/**
 * Apply `f` to a value that may or may not have arrived yet. The emitted store queries chain
 * through it: an in-memory handle returns its values at once and `f` runs at once; a handle
 * backed by storage returns a promise and `f` runs when it settles. (A store query is a
 * deferred producer whose elements are either, #3189.)
 */
function isThenable<T>(value: T | Promise<T>): value is Promise<T> {
  return value !== null && (typeof value === "object" || typeof value === "function") && typeof (value as { then?: unknown }).then === "function";
}

export function then<T, R>(value: T | Promise<T>, f: (value: T) => R | Promise<R>): R | Promise<R> {
  return isThenable(value) ? value.then(f) : f(value);
}

/**
 * The commit-time check of `@unique(by:)` (#4038): for each `[field, index, location]`, no value of the
 * index may be held by two entries of the staged map. A duplicate refuses the commit, as an invariant
 * violation does, naming the agent and `field.index`, never the value.
 */
export async function checkUnique(
  agent: string,
  state: object,
  declared: ReadonlyArray<readonly [string, string, string]>,
): Promise<void> {
  const handles = state as Readonly<Record<string, { hasDuplicate(index: string): boolean | Promise<boolean> }>>;
  for (const [field, index, location] of declared) {
    if (await (handles[field] as { hasDuplicate(index: string): boolean | Promise<boolean> }).hasDuplicate(index)) {
      throw invariantViolation(agent, `${field}.${index}`, "unique", location);
    }
  }
}

/**
 * The `@unique(by:)` values that more than one entry already holds (#4390), for `bynk migrate sweep`: for each
 * `[field, index]` the staged map has, the holders of each such value (`[2, 2, 4]`: three values). A map with none
 * is left out. A value is never named: the count of holders is all a report may carry.
 */
export async function uniqueDuplicates(
  state: object,
  declared: ReadonlyArray<readonly [string, string]>,
): Promise<Array<{ map: string; by: string; holders: number[] }>> {
  const handles = state as Readonly<Record<string, { duplicateHolders(index: string): number[] | Promise<number[]> }>>;
  const out: Array<{ map: string; by: string; holders: number[] }> = [];
  for (const [field, index] of declared) {
    const holders = [...(await (handles[field] as { duplicateHolders(index: string): number[] | Promise<number[]> }).duplicateHolders(index))].sort((a, b) => a - b);
    if (holders.length > 0) out.push({ map: field, by: index, holders });
  }
  return out;
}

// #1653: the `__` aliases emitted code imports.
export { commitStored as __commitStored, eraseStored as __eraseStored, markErased as __markErased, newErasure as __newErasure, checkUnique as __checkUnique, committedSize as __committedSize, storeKey as __bynkStoreKey, noteClock as __noteClock, moveRenamedEntries as __moveRenamedEntries, readStored as __readStored, readsLazily as __readsLazily, then as __bynkThen };
