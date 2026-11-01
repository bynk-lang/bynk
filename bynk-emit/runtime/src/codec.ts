// Constrained generics slice 3 (#2715): the codec combinators behind `Codable`.
//
// A `Codable`-bounded type parameter takes a hidden codec ("witness") built at
// the call site from the concrete type's generated `__serialise_X` /
// `__deserialise_X` pair. Inside a generic body a built-in container over such a
// parameter (`List[T]`, `Option[T]`, `Set[T]`, `Map[K, T]`, `Result[T, E]`) gets
// its codec from the combinators below, over the witnesses of its parts. Each one
// produces exactly what the monomorphised helper produces for the same concrete
// type: the same JSON, the same errors at the same paths. A test pins this per
// container.
//
// Internal, not part of the published ABI (ADR 0310): a capability provider
// never receives a witness, only the encoded JSON.
import type { Option, Result } from "./result.ts";
import { Ok, Err, Some, None } from "./result.ts";
import type { BoundaryError, JsonValue } from "./boundary.ts";
import { __BynkKeyMap, __BynkKeySet } from "./keys.ts";

/** An encode/decode pair for one concrete type. */
export interface __BynkCodec<T> {
  encode: (v: T) => JsonValue;
  decode: (j: JsonValue, path: string) => Result<T, BoundaryError>;
}

function mismatch(path: string, expected: string, actual: string): Result<never, BoundaryError> {
  return Err({ kind: "StructuralMismatch", path, expected, actual });
}

function plainKey(k: unknown): boolean {
  return typeof k !== "object" || k === null;
}

export function __bynkCodecList<T>(c: __BynkCodec<T>): __BynkCodec<readonly T[]> {
  return {
    encode: (v) => v.map((x) => c.encode(x)),
    decode: (json, path) => {
      if (!Array.isArray(json)) return mismatch(path, "array", typeof json);
      const out: T[] = [];
      for (let i = 0; i < json.length; i++) {
        const r = c.decode(json[i], `${path}[${i}]`);
        if (r.tag === "Err") return r;
        out.push(r.value);
      }
      return Ok(out);
    },
  };
}

const OPTION_FORMS_SUFFIX = ' | null | {"kind": "None"} | {"kind": "Some", "value": ...}';

export function __bynkCodecOption<T>(c: __BynkCodec<T>): __BynkCodec<Option<T>> {
  return {
    encode: (v): JsonValue => {
      if (v.tag === "Some") return { kind: "Some", value: c.encode(v.value) };
      return { kind: "None" };
    },
    decode: (json, path) => {
      if (json === undefined || json === null) return Ok(None as Option<T>);
      if (typeof json === "object" && !Array.isArray(json)) {
        const obj = json as { [k: string]: JsonValue };
        const keys = Object.keys(obj);
        if (obj["kind"] === "None" && keys.length === 1) return Ok(None as Option<T>);
        if (obj["kind"] === "Some" && keys.length === 2 && "value" in obj) {
          const t = c.decode(obj["value"], `${path}.value`);
          if (t.tag === "Err") return t;
          return Ok(Some(t.value) as Option<T>);
        }
      }
      const b = c.decode(json, path);
      if (b.tag === "Ok") return Ok(Some(b.value) as Option<T>);
      if (
        b.error.kind === "StructuralMismatch" &&
        b.error.path === path &&
        !b.error.expected.endsWith(OPTION_FORMS_SUFFIX)
      ) {
        return Err({ ...b.error, expected: b.error.expected + OPTION_FORMS_SUFFIX });
      }
      return b;
    },
  };
}

export function __bynkCodecSet<T>(c: __BynkCodec<T>): __BynkCodec<ReadonlySet<T>> {
  return {
    encode: (v) => Array.from(v).map((x) => c.encode(x)),
    decode: (json, path) => {
      if (!Array.isArray(json)) return mismatch(path, "array", typeof json);
      // The keyed set is chosen by the first element, as `__bynkSetOf` does: all
      // elements of one static `Key` type are primitive or all composite.
      let out: Set<T> | undefined;
      for (let i = 0; i < json.length; i++) {
        const r = c.decode(json[i], `${path}[${i}]`);
        if (r.tag === "Err") return r;
        out ??= plainKey(r.value) ? new Set<T>() : new __BynkKeySet<T>();
        if (out.has(r.value)) return mismatch(`${path}[${i}]`, "a distinct set element", "a duplicate");
        out.add(r.value);
      }
      return Ok(out ?? new Set<T>());
    },
  };
}

export function __bynkCodecMap<K, V>(ck: __BynkCodec<K>, cv: __BynkCodec<V>): __BynkCodec<ReadonlyMap<K, V>> {
  return {
    encode: (m) => {
      const entries: JsonValue[] = [];
      for (const [k, v] of m) entries.push([ck.encode(k), cv.encode(v)]);
      return entries;
    },
    decode: (json, path) => {
      if (!Array.isArray(json)) return mismatch(path, "array", typeof json);
      let out: Map<K, V> | undefined;
      for (let i = 0; i < json.length; i++) {
        const entry = json[i];
        if (!Array.isArray(entry) || entry.length !== 2) {
          return mismatch(`${path}[${i}]`, "[key, value] entry", typeof entry);
        }
        const rk = ck.decode(entry[0], `${path}[${i}][0]`);
        if (rk.tag === "Err") return rk;
        const rv = cv.decode(entry[1], `${path}[${i}][1]`);
        if (rv.tag === "Err") return rv;
        out ??= plainKey(rk.value) ? new Map<K, V>() : new __BynkKeyMap<K, V>();
        out.set(rk.value, rv.value);
      }
      return Ok(out ?? new Map<K, V>());
    },
  };
}

export function __bynkCodecResult<T, E>(ct: __BynkCodec<T>, ce: __BynkCodec<E>): __BynkCodec<Result<T, E>> {
  return {
    encode: (v): JsonValue => {
      if (v.tag === "Ok") return { kind: "Ok", value: ct.encode(v.value) };
      return { kind: "Err", error: ce.encode(v.error) };
    },
    decode: (json, path) => {
      if (typeof json !== "object" || json === null || Array.isArray(json)) {
        return mismatch(path, "object", typeof json);
      }
      const obj = json as { [k: string]: JsonValue };
      if (obj["kind"] === "Ok") {
        const r = ct.decode(obj["value"], `${path}.value`);
        if (r.tag === "Err") return r;
        return Ok(Ok(r.value) as Result<T, E>);
      }
      if (obj["kind"] === "Err") {
        const r = ce.decode(obj["error"], `${path}.error`);
        if (r.tag === "Err") return r;
        return Ok(Err(r.value) as Result<T, E>);
      }
      return mismatch(path, "Ok | Err", String(obj["kind"]));
    },
  };
}
