---
level: patch
probes: ts_writes
changelog: "A boundary codec bounds the nesting of a recursive type at 256 levels: decoding a value nested more deeply is a `StructuralMismatch` (an HTTP `400`, an `Err` from `Json.decode`) instead of a stack overflow the Worker answered as a fault, and encoding one faults ([#2656](https://github.com/bynk-lang/bynk/issues/2656))"
---

## ADR: recursive-nesting-bound
title: A boundary codec bounds recursive nesting at 256 levels
summary: The codecs of a recursive type thread a depth; past 256 levels decoding is a `StructuralMismatch` and encoding throws, and every other codec is unchanged

**Context.** A non-generic recursive type is boundary-legal (§6.5), and its
generated `__deserialise_<T>(json, path)` calls itself once per level of
nesting, with no limit. A deep enough value exhausts the host's stack, and the
`RangeError` is not a `__BoundaryError`, so it escapes the decoder. In a
`workers` entry point the outer `catch` logs it, reports a fault and answers
with the fault response. A `from http` route with `by Visitor` whose body type
is recursive therefore let an unauthenticated client trigger faults at will. The
same decoders serve `/_bynk/call/` arguments and results, event payloads, agent
state rehydration and `Json.decode[T]`.

**Decision.**

- **A. A nesting bound, enforced only where nesting is unbounded.** Only a type
  on a type cycle can nest without a limit the program text sets.
  `bynk_check::wire::recursive_codec_suffixes` builds the graph of codec calls
  (nodes are codec suffixes: named types, generic instantiations, `Option`,
  `List`, `Set`, `Map`, `Result`) from every non-generic declaration and returns
  the nodes on a cycle (Tarjan's strongly connected components; a component
  larger than one node, or a self-call). Those codecs, and only those, take a
  trailing `depth: number = 0`. A record's or sum's codec (generic
  instantiations included) counts one level and passes `depth + 1` to callees
  on a cycle; a container's codec passes `depth` through. The `Option` value
  decoder gains a required `depth` when its `Option` is on a cycle. Every other
  codec, and every call from outside a cycle (an entry point, `Json.decode`, a
  non-recursive record holding a recursive field), is emitted byte for byte as
  before, so only fixtures with recursive boundary types change.
  - A value of a non-recursive type between two recursive ones is not counted,
    and as its codec carries no depth the count starts again below it. Such a
    type occurs a fixed number of times on any path, so the total stays bounded
    by the program; threading the depth through it would have changed every
    non-recursive codec.
  - **Rejected:** a depth on every codec, which would re-bless every golden for
    no gain; counting every codec frame (containers too), which tracks stack use
    a little more evenly but makes "levels" mean nothing to a Bynk reader.
- **B. Decoding past the bound is a boundary error.** The first statement of a
  recursive type's decoder is
  `if (depth >= 256) return Err({ kind: "StructuralMismatch", path, expected: "a value nested at most 256 levels deep", actual: "a value nested more deeply" })`,
  at the path of the value that crosses the bound. Being a `__BoundaryError`,
  it takes every consumer's existing error path: a `400` for an HTTP body or a
  `/_bynk/call/` argument, `Err(JsonError { kind: "StructuralMismatch", … })`
  from `Json.decode`, the boundary-error path of event decoding. A lenient
  `Option` decoder on a cycle does not append its list of accepted forms to
  this error, since the error is not about the `Option`. No new
  `BoundaryError` kind was added, so the published error bodies are unchanged.
- **C. Encoding past the bound throws**, as encoding a non-finite `Float` does
  (§7.2): `value nested more than 256 levels deep at boundary`. A context
  therefore never sends a value its peer must refuse, and never stores one its
  own rehydration must refuse: a stored value deeper than the bound cannot be
  written, so rehydration never meets one.
- **D. The bound is 256, chosen by measurement.** On Node 22, the emitted
  `--target workers` codecs compiled with `tsc` and imported, each probe in a
  fresh process (cold, interpreted frames, the worst case), the first level
  count that overflowed was:

  | Shape | Decode, before | Encode, before | Decode, after | Encode, after |
  |---|---|---|---|---|
  | `Tree`: a sum recursing directly | 4,179 | 9,277 | 4,179 | 7,094 |
  | `Dir`: a record through `List` | 2,090 | 2,061 | 2,090 | 1,796 |
  | `Chain`: a record through `Option` | 2,090 | 4,834 | 1,885 | 3,809 |
  | `MapNode`: a record through `Map` | 1,394 | 2,733 | 1,394 | 2,372 |
  | `Deep`: a record through `Map[String, List[Option[_]]]` | 837 | 1,176 | 837 | 1,031 |

  "After" is the depth-threaded code with the bound lifted, to show the extra
  parameter costs little stack. The smallest of the four reference shapes is
  1,394 levels (`MapNode`, decoding). The Workers runtime's stack can be smaller
  than Node's, so the bound is at most a quarter of that (348), and at least 256
  so that legitimately deep data, such as a long linked list, still crosses: 256.
  It is one constant, `bynk_check::wire::MAX_RECURSIVE_NESTING`, which the
  emitter, the wire-format reference and the tests read.
  - The `Deep` shape, with three containers per level, overflows at 837: 3.3
    times the bound rather than 5.4. A type nesting even more containers per
    level could, on a host with a much smaller stack than Node's, still
    overflow below 256 levels. Bounding stack use exactly would mean counting
    frames, which Decision A rejects.
- **E. The level is `patch`.** It is a fix: input that faulted now gets the
  `400` the specification already promised for malformed input, and no program
  is accepted or rejected differently by the checker. `minor` would be arguable
  because §6.5 gains a normative rule and a value between 257 levels and the
  host's overflow depth, which decoded before, is now refused; but recursive
  types at boundaries were specified only in #2613, with the depth explicitly
  host-defined, and this sets that host-defined depth.
- **F. The wire format stays at version 1.** The reference documents the bound
  under *Encoding*. Its stability rule makes a changed check a new version, but
  the reference never promised a nesting depth, §6.5 called it host-defined,
  the error is an existing kind with existing fields, and a Bynk sender with
  this fix never sends a value past the bound. A pre-fix Bynk sender could send
  a value 257 to roughly 1,400 levels deep that a fixed callee now refuses with
  a `400`; that skew is the one observable difference.

**Consequences.** A recursive body nested past 256 levels is refused before it
can exhaust the stack, so an unauthenticated client can no longer fault a
Worker this way. Generated code changes only for types on a cycle. A
legitimate value deeper than 256 levels can no longer cross a boundary; a
program that needs one must flatten it, for example into a `List`.
