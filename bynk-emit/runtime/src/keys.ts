// Constrained generics slice 2 (#2684): composite keys for a value `Map`/`Set`.
//
// A JS `Map`/`Set` compares keys by SameValueZero, so two equal records are two
// keys; Bynk's `==` is structural (ADR 0421). A collection whose key is a
// primitive (`string`, `number`, `boolean`) stays a plain `Map`/`Set` and every
// site that is statically primitive-keyed emits exactly what it always did. A
// collection that meets its first composite key (a record, sum, `Option`,
// `List` or `Bytes`) is promoted to `__BynkKeyMap`/`__BynkKeySet`, which extend
// `Map`/`Set`, keep the original keys (so `keys()` and iteration return real
// keys, in insertion order) and index them by `__bynkKeyEncode`.
//
// The choice is value-directed rather than type-directed because Bynk generics
// are erased: `bynk.set.union[T]` builds its result with no knowledge of `T`.
// All keys of one static `Key` type are either all primitive or all objects (an
// `Option`/sum is always an object, a record always an object), so promotion
// happens at the first insert into an empty collection.
//
// `__bynkEq` tests `instanceof Map`/`instanceof Set` and reads through `has` and
// `get`, so it compares the subclasses structurally without change.
//
// Everything here is internal, not part of the published ABI (ADR 0310).
// `serialiseAgentKey` (agent.ts) is deliberately a separate function: it
// addresses stored durable objects and must stay byte-identical.

/**
 * The canonical form of a key value. Two `Key` values of one static type are
 * `==` exactly when their canonical forms are equal: strings, numbers and
 * booleans are themselves; `Bytes` is its lowercase hex string; an array is
 * canonicalised element-wise; any other object is its own keys, sorted, each
 * canonicalised. A sum is `{ tag, … }`, so the variant tag takes part, and a
 * nested string is quoted by `JSON.stringify`, so `"1"` and `1` stay distinct.
 * `-0` and `0` encode alike, matching `==`; a `Float` never reaches the encoder.
 */
function canonical(v: unknown): unknown {
  if (v instanceof Uint8Array) {
    let hex = "";
    for (let i = 0; i < v.length; i++) hex += (v[i] < 16 ? "0" : "") + v[i].toString(16);
    return hex;
  }
  if (Array.isArray(v)) return v.map(canonical);
  if (typeof v === "object" && v !== null) {
    const obj = v as { [k: string]: unknown };
    const out: { [k: string]: unknown } = {};
    for (const k of Object.keys(obj).sort()) out[k] = canonical(obj[k]);
    return out;
  }
  return v;
}

/** The canonical string a collection indexes a key by. */
export function __bynkKeyEncode(v: unknown): string {
  return JSON.stringify(canonical(v));
}

/** Whether `k` can key a plain `Map`/`Set` (SameValueZero is value equality). */
function plainKey(k: unknown): boolean {
  return typeof k !== "object" || k === null;
}

/** A `Map` indexed by the canonical encoding of its keys. */
export class __BynkKeyMap<K, V> extends Map<K, V> {
  private idx = new Map<string, K>();

  constructor(entries?: Iterable<[K, V]> | null) {
    super();
    if (entries instanceof __BynkKeyMap) {
      // A copy reuses the index instead of encoding every key again.
      for (const [k, v] of entries) super.set(k, v);
      this.idx = new Map(entries.idx);
    } else if (entries) {
      for (const [k, v] of entries) this.set(k, v);
    }
  }

  override set(k: K, v: V): this {
    const enc = __bynkKeyEncode(k);
    if (this.idx.has(enc)) {
      // The first spelling of the key keeps its place and identity.
      super.set(this.idx.get(enc) as K, v);
    } else {
      this.idx.set(enc, k);
      super.set(k, v);
    }
    return this;
  }

  override get(k: K): V | undefined {
    const enc = __bynkKeyEncode(k);
    return this.idx.has(enc) ? super.get(this.idx.get(enc) as K) : undefined;
  }

  override has(k: K): boolean {
    return this.idx.has(__bynkKeyEncode(k));
  }

  override delete(k: K): boolean {
    const enc = __bynkKeyEncode(k);
    if (!this.idx.has(enc)) return false;
    super.delete(this.idx.get(enc) as K);
    this.idx.delete(enc);
    return true;
  }

  override clear(): void {
    super.clear();
    this.idx.clear();
  }
}

/** A `Set` indexed by the canonical encoding of its elements. */
export class __BynkKeySet<T> extends Set<T> {
  private idx = new Map<string, T>();

  constructor(values?: Iterable<T> | null) {
    super();
    if (values instanceof __BynkKeySet) {
      for (const v of values) super.add(v);
      this.idx = new Map(values.idx);
    } else if (values) {
      for (const v of values) this.add(v);
    }
  }

  override add(v: T): this {
    const enc = __bynkKeyEncode(v);
    if (!this.idx.has(enc)) {
      this.idx.set(enc, v);
      super.add(v);
    }
    return this;
  }

  override has(v: T): boolean {
    return this.idx.has(__bynkKeyEncode(v));
  }

  override delete(v: T): boolean {
    const enc = __bynkKeyEncode(v);
    if (!this.idx.has(enc)) return false;
    super.delete(this.idx.get(enc) as T);
    this.idx.delete(enc);
    return true;
  }

  override clear(): void {
    super.clear();
    this.idx.clear();
  }
}

/** `m` with `k` set to `v`: a copy that keeps `m`'s kind, promoted when `k` is composite. */
export function __bynkMapInsert<K, V>(m: ReadonlyMap<K, V>, k: K, v: V): ReadonlyMap<K, V> {
  const out =
    m instanceof __BynkKeyMap || !plainKey(k) ? new __BynkKeyMap<K, V>(m) : new Map<K, V>(m);
  return out.set(k, v);
}

/** `s` with `x` added: a copy that keeps `s`'s kind, promoted when `x` is composite. */
export function __bynkSetInsert<T>(s: ReadonlySet<T>, x: T): ReadonlySet<T> {
  const out =
    s instanceof __BynkKeySet || !plainKey(x) ? new __BynkKeySet<T>(s) : new Set<T>(s);
  return out.add(x);
}

/** `s` without `x`: a copy that keeps `s`'s kind. */
export function __bynkSetRemove<T>(s: ReadonlySet<T>, x: T): ReadonlySet<T> {
  const out = s instanceof __BynkKeySet ? new __BynkKeySet<T>(s) : new Set<T>(s);
  out.delete(x);
  return out;
}

/** A set of `xs`, in first-occurrence order, keyed by value. */
export function __bynkSetOf<T>(xs: Iterable<T>): ReadonlySet<T> {
  const list = [...xs];
  return list.some((x) => !plainKey(x)) ? new __BynkKeySet<T>(list) : new Set<T>(list);
}

/** `xs` without repeats, by value, keeping first occurrences in order. */
export function __bynkDistinct<T>(xs: Iterable<T>): T[] {
  return [...__bynkSetOf(xs)];
}

/** `xs` without repeats of `key(x)`, by value, keeping first occurrences in order. */
export function __bynkDistinctBy<T, K>(xs: Iterable<T>, key: (x: T) => K): T[] {
  const seen = new Set<string>();
  const out: T[] = [];
  for (const x of xs) {
    const enc = __bynkKeyEncode(key(x));
    if (!seen.has(enc)) {
      seen.add(enc);
      out.push(x);
    }
  }
  return out;
}

/** `List.groupBy`: partition by `key`, in first-seen key order, each group projected through `into`. */
export function __bynkGroupBy<T, K, V>(
  xs: Iterable<T>,
  key: (x: T) => K,
  into: (k: K, rows: readonly T[]) => V,
): V[] {
  const groups = new Map<string, T[]>();
  for (const x of xs) {
    const enc = __bynkKeyEncode(key(x));
    const rows = groups.get(enc);
    if (rows) rows.push(x);
    else groups.set(enc, [x]);
  }
  return [...groups.values()].map((rows) => into(key(rows[0]), rows));
}

function hashBy<U, K>(other: Iterable<U>, right: (u: U) => K): Map<string, U[]> {
  const h = new Map<string, U[]>();
  for (const u of other) {
    const enc = __bynkKeyEncode(right(u));
    const rows = h.get(enc);
    if (rows) rows.push(u);
    else h.set(enc, [u]);
  }
  return h;
}

/** `List.joinOn`: each left row with every right row of an equal key. */
export function __bynkJoinOn<T, U, K, V>(
  xs: Iterable<T>,
  other: Iterable<U>,
  left: (t: T) => K,
  right: (u: U) => K,
  into: (t: T, u: U) => V,
): V[] {
  const h = hashBy(other, right);
  return [...xs].flatMap((t) => (h.get(__bynkKeyEncode(left(t))) ?? []).map((u) => into(t, u)));
}

/** `List.leftJoin`: as `joinOn`, with `None` for a left row that has no match. */
export function __bynkLeftJoin<T, U, K, V>(
  xs: Iterable<T>,
  other: Iterable<U>,
  left: (t: T) => K,
  right: (u: U) => K,
  into: (t: T, u: { tag: "Some"; value: U } | { tag: "None" }) => V,
): V[] {
  const h = hashBy(other, right);
  return [...xs].flatMap((t) => {
    const m = h.get(__bynkKeyEncode(left(t))) ?? [];
    return m.length > 0
      ? m.map((u) => into(t, { tag: "Some", value: u }))
      : [into(t, { tag: "None" })];
  });
}
