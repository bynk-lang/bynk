---
level: minor
probes: emit_diagnostics, ts_writes
changelog: "`Set[T]` is a built-in immutable, insertion-ordered set beside `List` and `Map`: `Set.of(xs)`, `Set.empty()`, a `length`/`contains`/`insert`/`remove`/`toList` kernel, membership equality, and a JSON-array wire form that refuses a repeated element; members follow the `Map`-key rule (`bynk.types.unkeyable_set_element`), and a new `bynk.set` commons adds `union`, `intersect`, `difference`, `isSubsetOf`, `filterSet` and `mapSet` (standard-library K6a, [#2596](https://github.com/bynk-lang/bynk/issues/2596))"
---

## ADR: set-value-type
title: Set is a third built-in immutable collection
summary: `Set[T]` joins `List` and `Map` as a built-in generic (ADR 0035), insertion-ordered, with members confined to the `Map`-key rule, a five-method kernel plus `Set.of`/`Set.empty`, combinators in a `bynk.set` commons, membership equality, and a JSON array on the wire that refuses duplicates

**Context.** A set was a `Map[K, Bool]` or a deduplicated `List`, neither
saying what it meant, and `List.contains` is linear. ADR 0035 made `List` and
`Map` built-in because the boundary machinery must know their shape; a value
`Set` has the same need. The store kind `Set` (`store x: Set[T]`, ADR 0110) is
parsed by its own path, so a value type can share the name, as `Map` does.
`List` and `Map` are not lexer keywords, only names the type parser
recognises, so a built-in `Set` breaks no user `type Set`. A JS `Set` compares
by identity, so only value-comparable elements can be members.

**Decision.**

- **A. A built-in, immutable `Set[T]`, lowered to `ReadonlySet<T>`.** `Set`
  followed by `[` is the built-in; a bare `Set` stays an ordinary name in both
  the Rust parser and the tree-sitter grammar. It is insertion-ordered,
  normatively, as `Map` is: `toList` and the wire form list elements in the
  order they were first added, and inserting a present element keeps its
  place.
- **B. Members follow the `Map`-key rule** (ADR 0110 D5): `Int`, `String`, or a
  refined or opaque type over them. `bynk.types.unkeyable_set_element` is
  raised at a written `Set[T]` (the resolver) and at the inferred element of
  `Set.of` (the checker), since an inferred type never passes the former. A
  type variable is admitted, as for `Map` keys.
- **C. A minimal kernel.** `Set.empty()` (typed from context) and
  `Set.of(xs: List[T])` (the first occurrence keeps its position), then
  `length`, `contains`, `insert`, `remove` and `toList`. `insert` and `remove`
  copy, so a receiver is never mutated. These need a JS `Set`.
- **D. Combinators in Bynk.** `bynk.set` (`uses bynk.set`) has `union`,
  `intersect`, `difference`, `isSubsetOf`, `filterSet` and `mapSet`, each a
  fold or filter over `toList`. The last two are named apart from
  `bynk.list`'s `filter` and `map`: two used commons declaring one name is
  `bynk.uses.name_conflict`, with no renaming at the use site, so plain
  `filter` and `map` would stop a unit using both.
- **E. On the wire, a JSON array in insertion order, refusing duplicates.**
  The decoder fails on a repeated element (a `StructuralMismatch` at its path):
  a Bynk sender never writes one, so a duplicate means the payload is not the
  set it claims to be. **Rejected:** a silent dedupe, which would hide that.
  Contract compatibility requires the element type to match exactly, as a
  `Map` key's does. OpenAPI renders it as an array with `uniqueItems`.
- **F. Equality is membership.** `__bynkEq` compares size, then `has`, so two
  sets built in different orders are equal.
- **G. Stable.**

**Consequences.** A context can hold, compare and send a set directly. Elements
beyond the key rule (records, enums, `Bytes`, `Float`) need content hashing and
stay out. Set literals (`{a, b}`) would be grammar work; `Set.of([a, b])`
covers them.
