---
level: minor
probes: emit_diagnostics, diagnostic_coverage, explained_codes
changelog: `MapEntry` is a nameable prelude record, so `Map.of([MapEntry { key: k, value: v }])` builds a map from entries written in source
---

## ADR: map-entry-prelude-record
title: MapEntry becomes a nameable transparent prelude record
summary: Makes MapEntry[K, V] constructible and nameable in any unit, so Map.of can take entries written in source, and refuses it at a boundary

**Context.** `map.entries()` and a store `Map`'s `.entries` yield `MapEntry` values, but the name was not usable in source: `MapEntry { key: k, value: v }` and `List[MapEntry[K, V]]` were both `bynk.resolve.unknown_type`, so `Map.of` could only rebuild a map from another map's entries and never from computed pairs.

**Decision.** `MapEntry[K, V]` becomes a transparent prelude record, `{ key: K, value: V }`, that any unit can name and construct, following the ruling on #3948 that a transparent type is constructible outside its owner. It is written like any generic record: `MapEntry { key: k, value: v }` infers `K` and `V` from the fields, and `List[MapEntry[K, V]]` names the type. No `MapEntry.of` and no `Map.fromPairs` is added. The record has no declaration in any unit, so the lookups that resolve a type name fall back to a declaration parsed once from the prelude's own source (`prelude_record`); a type a unit declares or imports with the same name wins, as it did before. The existing checks apply unchanged (field set, argument arity, field type), so no new diagnostic is needed for them. The type is still the compiler-known one `entries()` yields, so it is the same `Ty` and renders in TypeScript as the inline `{ readonly key; readonly value }` object. A `MapEntry` is built and consumed in place and has no wire codec, so it is refused wherever a type is persisted or sent (a record, sum or event field, an agent key or store field, a handler's parameter or result) with the new error `bynk.types.map_entry_at_boundary`, as a `Query` is; a helper may take or return one.

**Consequences.** Additive: no program that compiled stops compiling, since the name was not in scope before, and a user type named `MapEntry` still takes precedence. The boundary refusal is deliberate and narrow: making `MapEntry` a boundary type needs a codec and a declared TypeScript name, which this change does not add, and a handler projects entries into a record of its own, as it already had to. The `TypeRef::App` arm of the boundary check moved out of `reject_fn_types` into its own module to keep that function within its length ceiling.
