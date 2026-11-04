# 0729 — Call-record types qualify `uses` commons types

- **Status:** Accepted (v0.504.3)

**Context.** `observation_call_record_types` qualified only the target's own types and consumed adapters', so `fn draw(p: Point)` with `Point` from a `uses` commons emitted an unqualified `Point` that `tsc` could not find.

**Decision.** The call-record emitter also maps each public type of every `uses` commons to that commons' test namespace, as the stub signatures already do. This covers records, refinements and enums, including nested in `List`/`Option`.

**Consequences.** `bynk check` and `bynk test` agree for commons types in capability operations. Stub, matcher and fake emission already used the qualified map.
