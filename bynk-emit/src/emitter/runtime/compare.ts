// Constrained generics slice 1 (#2660): the comparators behind `Ord`.
//
// A comparator is `(a, b) => -1 | 0 | 1`. Bynk generics are erased, so a
// `T: Ord` parameter cannot recover the order of its concrete type from the
// values: a nullary variant is a bare `{ tag }` with no position, and object key
// order is not declaration order. The emitter instead passes a hidden comparator
// ("witness") for each `Ord`-bounded parameter, built at the call site from the
// concrete type with the functions below. They are internal, not part of the
// published ABI (ADR 0310).
//
// The orders (track Q5):
// - `Int`, `Duration`, `Instant` are numbers of the same scale: numeric order.
// - `String` is UTF-16 code-unit order, which is what JS `<` does on strings.
// - `Float` is a total order that agrees with `==` except at `NaN`: `-0` and `0`
//   compare equal, and `NaN` compares equal to itself and greater than every
//   other value. The comparison operators keep their IEEE meaning (`NaN < x` is
//   `false`); only the ordering kernels and `Ord` witnesses use this.
// - `Bytes` is unsigned lexicographic (`__bynkBytesCompare`).
// - `Option` puts `None` first; `List` is lexicographic.
// - A record compares its fields in declaration order; a sum compares the
//   variant's declaration position first, then its payload fields in order.

import { __bynkBytesCompare } from "./bytes.ts";

/** A comparator: negative, zero or positive, as `a` sorts before, with or after `b`. */
export type __BynkCmp<T> = (a: T, b: T) => number;

/** `Int`, `Duration` and `Instant`. */
export function __bynkCmpInt(a: number, b: number): number {
  return a < b ? -1 : a > b ? 1 : 0;
}

/** `String`, in UTF-16 code-unit order. */
export function __bynkCmpString(a: string, b: string): number {
  return a < b ? -1 : a > b ? 1 : 0;
}

/** `Float`: the total order, with `NaN` last and equal to itself. */
export function __bynkCmpFloat(a: number, b: number): number {
  const aNaN = a !== a;
  const bNaN = b !== b;
  if (aNaN || bNaN) return aNaN ? (bNaN ? 0 : 1) : -1;
  return a < b ? -1 : a > b ? 1 : 0;
}

/** `Bytes`. */
export function __bynkCmpBytes(a: Uint8Array, b: Uint8Array): number {
  return __bynkBytesCompare(a, b);
}

type OptionLike<T> = { readonly tag: "Some"; readonly value: T } | { readonly tag: "None" };

/** `Option[T]`: `None` sorts first. */
export function __bynkCmpOption<T>(c: __BynkCmp<T>): __BynkCmp<OptionLike<T>> {
  return (a, b) => {
    if (a.tag === "None") return b.tag === "None" ? 0 : -1;
    if (b.tag === "None") return 1;
    return c(a.value, b.value);
  };
}

/** `List[T]`: lexicographic; a proper prefix sorts first. */
export function __bynkCmpList<T>(c: __BynkCmp<T>): __BynkCmp<readonly T[]> {
  return (a, b) => {
    const n = Math.min(a.length, b.length);
    for (let i = 0; i < n; i++) {
      const r = c(a[i], b[i]);
      if (r !== 0) return r;
    }
    return a.length === b.length ? 0 : a.length < b.length ? -1 : 1;
  };
}

/** One compared field: its property name and its comparator. */
export type __BynkCmpField = readonly [string, __BynkCmp<any>];

function cmpFields(fields: readonly __BynkCmpField[], a: any, b: any): number {
  for (const [name, c] of fields) {
    const r = c(a[name], b[name]);
    if (r !== 0) return r;
  }
  return 0;
}

/** A record `derives Ord`: its fields, in declaration order. */
export function __bynkCmpRecord<T>(fields: readonly __BynkCmpField[]): __BynkCmp<T> {
  return (a, b) => cmpFields(fields, a, b);
}

/**
 * A sum `derives Ord`: `order` lists the variants in declaration order and
 * `payloads` gives each variant's compared fields (a nullary variant has none).
 */
export function __bynkCmpSum<T extends { readonly tag: string }>(
  order: readonly string[],
  payloads: { readonly [tag: string]: readonly __BynkCmpField[] },
): __BynkCmp<T> {
  return (a, b) => {
    if (a.tag !== b.tag) return order.indexOf(a.tag) < order.indexOf(b.tag) ? -1 : 1;
    const fields = payloads[a.tag];
    return fields ? cmpFields(fields, a, b) : 0;
  };
}

/**
 * Close a comparator over itself: `build` receives a comparator that delegates
 * to the one it returns, so a recursive type (`Tree = Leaf | Node(l: Tree, …)`)
 * can name itself while its comparator is still being built.
 */
export function __bynkCmpFix<T>(build: (self: __BynkCmp<T>) => __BynkCmp<T>): __BynkCmp<T> {
  let built: __BynkCmp<T> | undefined;
  const self: __BynkCmp<T> = (a, b) => built!(a, b);
  built = build(self);
  return built;
}

