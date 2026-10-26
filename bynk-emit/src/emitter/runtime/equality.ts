// #1652 (runtime-semantics track §3.1): structural `==` for every type the
// emitter cannot compare with a bare `===`.
//
// The emitter dispatches `==` by the operand's type (`lower_bin_op`, the same
// operand-typed shape as `Int`/`Float` division):
// - a statically primitive operand (a base type other than `Bytes`, or a
//   refined/opaque type over one) keeps `===`;
// - `Bytes`, including an opaque type over it, uses `__bynkBytesEqual`;
// - everything else uses `__bynkEq`.
//
// A type-directed `equals_T` per type was rejected: Bynk generic functions are
// not monomorphised (they emit as TS generics), so `==` on a type parameter has
// no concrete type to dispatch on. The in-memory representation describes
// itself instead, so this walker needs no type information:
//
// - primitives compare with `===`, keeping IEEE semantics: `NaN != NaN` and
//   `-0 == 0`, at any depth;
// - a `Uint8Array` (`Bytes`) compares by content;
// - an array (`List`) compares element-wise;
// - a `Map` (value-level `Map`) compares by size, then by key and value;
// - a plain object (a record, a sum value `{ tag, … }`, an `Option`/`Result`)
//   compares by its own keys and their values, so `tag` decides first;
// - an instance of any other class compares by identity. The checker rejects
//   `==` on the only such values a program holds (held connections), so this
//   is a fallback, not a semantic.
//
// The checker guarantees both operands have the same type, so shapes can only
// differ by variant or by value, never by kind.
import { __bynkBytesEqual } from "./bytes.ts";

export function __bynkEq(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (typeof a !== "object" || typeof b !== "object" || a === null || b === null) {
    return false;
  }
  if (a instanceof Uint8Array || b instanceof Uint8Array) {
    return a instanceof Uint8Array && b instanceof Uint8Array && __bynkBytesEqual(a, b);
  }
  if (Array.isArray(a) || Array.isArray(b)) {
    if (!Array.isArray(a) || !Array.isArray(b) || a.length !== b.length) return false;
    for (let i = 0; i < a.length; i++) {
      if (!__bynkEq(a[i], b[i])) return false;
    }
    return true;
  }
  if (a instanceof Map || b instanceof Map) {
    if (!(a instanceof Map) || !(b instanceof Map) || a.size !== b.size) return false;
    for (const [k, v] of a) {
      if (!b.has(k) || !__bynkEq(v, b.get(k))) return false;
    }
    return true;
  }
  const pa = Object.getPrototypeOf(a);
  const pb = Object.getPrototypeOf(b);
  const plain = (p: unknown) => p === Object.prototype || p === null;
  if (!plain(pa) || !plain(pb)) return false;
  const ka = Object.keys(a);
  const kb = Object.keys(b);
  if (ka.length !== kb.length) return false;
  for (const k of ka) {
    if (!Object.prototype.hasOwnProperty.call(b, k)) return false;
    if (!__bynkEq((a as Record<string, unknown>)[k], (b as Record<string, unknown>)[k])) {
      return false;
    }
  }
  return true;
}

