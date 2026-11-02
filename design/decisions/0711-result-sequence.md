# 0711 — List sequenceAll and sequenceTry, pure Result accumulation

- **Status:** Accepted (v0.489)

**Context.** `.of` short-circuits and `ValidationError` is singular, so validating a whole form and reporting every failing field had no idiomatic spelling. Only the effectful collectors existed: `traverseAll` (gather every outcome, ADR 0172) and `traverseTry` (stop at the first `Err`, ADR 0174), both over `f: T -> Effect[Result[U, E]]`. There was no pure `List[Result[T, E]] -> Result[List[T], List[E]]`.

**Decision.** The maintainer chose (2026-11-02) a pure kernel method on `List[Result[T, E]]` that collects every error, plus a first-error companion.

- *Names.* `sequenceAll()` returns `Result[List[T], List[E]]`; `sequenceTry()` returns `Result[List[T], E]`. They mirror `traverseAll`/`traverseTry`'s suffixes (`All` gathers every outcome, `Try` stops at the first `Err`) with the function removed (`traverse` with identity is `sequence`). `sequenceAll` returns one `Result` of all errors where `traverseAll` returns the `List[Result]` unchanged. `collectAll`/`collectTry` were rejected because `collect` already names the `Query` terminal.
- *Semantics.* Errors in input order; any `Err` discards the `Ok` values; the empty list is `Ok([])`. Pure: nothing runs a function value, so no effect confinement.
- *Receiver.* A non-`Result` element is the new `bynk.types.sequence_needs_result`, because `combinator_return_mismatch` is about a function's return type and there is no function here.
- *Not in this slice.* `Query`/`Map` receivers (a `Query` is `collect`ed first), a `Validated` applicative type, and a record-construction form that collects each field's `.of` failure.
- Lowering: one inline loop each, typed from the call's checked type; no runtime helper.

**Consequences.** Checker: one kernel arm, two registry entries (hover, completion, signature help), one diagnostic. Emitter: two inline loops. No grammar, runtime, wire or ABI change. Fixtures pin all-Ok, mixed, all-Err and the empty list for both methods, and the non-`Result` element.
