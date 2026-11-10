import type { DurableObjectListOptions, DurableObjectStorage, DurableObjectTransaction } from "./storage.ts";
import { STORAGE_BATCH_LIMIT, utf8Compare } from "./storage.ts";

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

/** One posting of a secondary index: `by` is the indexed value, `pk` the primary key. */
export const postingKey = (field: string, index: string, by: string, pk: string): string =>
  `i/${encodeKeyPart(field)}/${encodeKeyPart(index)}/${encodeKeyPart(by)}/${encodeKeyPart(pk)}`;
/** Every posting of one indexed value; with `by` omitted, every posting of the index. */
export const postingPrefix = (field: string, index: string, by?: string): string =>
  `i/${encodeKeyPart(field)}/${encodeKeyPart(index)}/${by === undefined ? "" : `${encodeKeyPart(by)}/`}`;

/** A `Cache` expiry entry, ordered by expiry time so reaping is a bounded range read. */
export const expiryKey = (field: string, exp: number, k: string): string =>
  `x/${encodeKeyPart(field)}/${padNumber(exp, TIME_WIDTH)}/${encodeKeyPart(k)}`;
export const expiryPrefix = (field: string): string => `x/${encodeKeyPart(field)}/`;
/** The expiry entries with `exp <= now`, as `list` options. */
export function expiredRange(field: string, now: number): DurableObjectListOptions {
  const prefix = expiryPrefix(field);
  return { prefix, end: `${prefix}${padNumber(now + 1, TIME_WIDTH)}/` };
}

/** Per-collection metadata: the entry count for `size()` and a `Log`'s counter. */
export const metaKey = (field: string): string => `c/${encodeKeyPart(field)}`;

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
}

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
        entries[metaKey(f.name)] = { size: Object.keys(inline as object).length };
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

