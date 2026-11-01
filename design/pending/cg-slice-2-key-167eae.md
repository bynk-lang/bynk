---
level: minor
probes: emit_diagnostics, diagnostic_coverage, ts_writes, ts_any
changelog: "**`Key`: structural `Map` keys and `Set` members** (constrained-generics slice 2, breaking for generics over maps and sets): records, sums, enums, `Bytes`, and `Option`/`List` of a key are now legal `Map` keys, `Set` members and `distinct`/`groupBy`/join keys (`Map[OrderId, Int]`, `Set[Status]`, `Map[Bytes, V]`); a `Float` never is, at any depth. `Key` is automatic, implies `Eq`, and is a bound (`fn index[K: Key, V](…)`); it is the one bound a capability operation may carry. **A type parameter in a `Map` key or `Set` element position must now be declared `K: Key`**: run `bynk check --fix` to migrate (the stdlib's `bynk.map` and `bynk.set` are migrated). New diagnostics `bynk.types.not_keyable` (replaces `unkeyable_map_key`, `unkeyable_set_element` and `unkeyable_distinct`) and `bynk.generics.missing_key_bound` (machine-applicable). Store `Map`/`Set`/`Cache` and `@indexed` keys, `Query` keys and agent keys are unchanged"
---

## ADR: key-structural-map-and-set-keys
title: `Key` admits structural types as `Map` keys and `Set` members, held under a canonical encoding
summary: the `Key` constraint, its structural rule, `K: Key` on type parameters in key position, promote-on-first-composite-key runtime collections and the canonical key encoding (constrained-generics slice 2)

**Context.** A `Map` key or `Set` member had to be `String`, `Int`, or a refined or
opaque type over them, because a JS `Map`/`Set` compares keys by SameValueZero and
two equal records are two keys. Bynk's `==` is structural (ADR 0421), so a
collection must agree with it, and only the language can make lookup, `distinct`,
`groupBy`, joins and the wire codecs share one notion of "same key". The rule lived
in three places (the resolver's `check_keyable`, the checker's `is_keyable` and
the store's `type_ref_is_keyable`), and an unbounded type parameter was admitted
in key position in some of them and rejected in others. This is slice 2 (#2684) of
the constrained-generics track (`design/tracks/constrained-generics.md`, Q6 and §8),
which builds on slice 0's bounds and slice 1's `Ord`. The maintainer approved
#2684 as written, the Recommended option for every decision below. It amends ADR
0421's note that structural keys are a possible follow-on, and supersedes the
key-rule text of ADRs 0038, 0142 and 0658.

**Decision.**
1. **What is `Key` (A).** `String`, `Int`, `Bool`, `Instant`, `Duration`, `Bytes`,
   and refined or opaque types over them; `Option[T]` and `List[T]` of a `Key`;
   records, sums and enums whose every field and payload is `Key`, automatically,
   with no `derives`; a generic record or sum at an instantiation whose arguments
   are; a type parameter declared `K: Key`. A `Float` is never `Key`, at any
   depth, with a note naming `NaN`. `Map`, `Set`, functions, held resources,
   streams, actors and boundary-only types are not. The rule is one predicate
   (`checker/keying.rs`, `not_keyable`) that replaces the resolver's and checker's
   copies. The three codes `unkeyable_map_key`, `unkeyable_set_element` and
   `unkeyable_distinct` become `bynk.types.not_keyable` with a site-specific
   message, and are retired.
2. **Runtime representation (B).** A value `Map`/`Set` whose keys are all
   primitive stays a plain JS `Map`/`Set`, and every site whose key type is
   statically primitive emits byte-identical code. At a site whose key type is
   composite or a type parameter, the kernels call helpers in a new `keys` runtime
   module (`__bynkMapInsert`, `__bynkSetInsert`, `__bynkSetRemove`, `__bynkSetOf`,
   `__bynkDistinct`, `__bynkDistinctBy`, `__bynkGroupBy`, `__bynkJoinOn`,
   `__bynkLeftJoin`). The first composite key promotes the collection to
   `__BynkKeyMap`/`__BynkKeySet`, subclasses that keep the original keys (so
   `keys()` and iteration return real keys, in insertion order) and index them by
   the canonical encoding. The choice is value-directed because generics are
   erased: `bynk.set.union[T]` knows nothing of `T`. `__bynkEq` compares the
   subclasses structurally through `has` and `get`, unchanged.
3. **The canonical encoding (C).** `__bynkKeyEncode(v)` is `JSON.stringify` of a
   canonical tree: strings, numbers and booleans as themselves, a `Uint8Array` as
   its lowercase hex string, an array element-wise, any other object as its own
   keys sorted. It is injective per static `Key` type (two values encode equal
   exactly when they are `==`), which fixes both flaws of `serialiseAgentKey`:
   nested strings are quoted and `Bytes` is compact. `-0` and `0` encode alike.
4. **Agent keys (D).** `serialiseAgentKey` is unchanged: it feeds `idFromName` and
   addresses stored durable objects. The new encoder is a separate function.
5. **Store keys (E).** Store `Map`/`Set`/`Cache` and `@indexed` keys keep their
   rule and codes (`bynk.store.unkeyable_key`, `bynk.index.unkeyable_key`): they
   persist as JSON objects keyed by `String(k)`, and a composite key would be a new
   frozen encoding. A value `Map` inside a `Cell` or record takes composite keys
   now, because it rides the boundary codec additively (an entries array).
6. **Type parameters (F).** A type parameter in a `Map` key or `Set` element
   position, in a signature, a generic type's field, or a key of `distinct`,
   `groupBy` or a join, must be declared `K: Key`
   (`bynk.generics.missing_key_bound`, machine-applicable: it declares the bound,
   replacing an explicit `Eq`). A call must instantiate each `Key`-bounded
   parameter with a key type. This retires the type-parameter exemptions. `Key` is
   allowed on a capability operation's type parameter, alone among the
   constraints, since it adds no hidden argument and nothing to the ABI.
7. **`Key` implies `Eq` (G).** `Ord` does not imply `Key`. `T: Eq + Key` is the
   warning `bynk.generics.redundant_eq_bound`; `T: Ord + Key` is legitimate.
8. **Boundary decoding (H).** The `Set` decoder builds a keyed set for a composite
   element, so a repeated member is rejected by value. The `Map` decoder keeps
   last-entry-wins (a change to it would alter existing primitive maps; #2707).
9. **`Query` (I).** `Query`'s `distinct`, `distinctBy`, `groupBy` and join keys keep
   the `String`/`Int` rule on their own predicate, as slice 1 did for its ordering.

**Consequences.** `Map[OrderId, Int]`, `Set[Status]`, `Map[Bytes, V]` and
`distinct` over records work, and `fn index[K: Key, V](…)` is writable. Generics
over a `Map` or `Set` need a `Key` bound: breaking, with a machine-applicable fix
that `bynk check --fix` applies, and the stdlib is migrated. Primitive-keyed code
is untouched at run time; a composite-keyed collection pays one encoding per
operation. Composite store keys, `Query` keys, a `Map`/`Set` as a key, and a
duplicate-key check in the `Map` decoder are separate, additive increments.
