---
level: minor
probes: emit_diagnostics, diagnostic_coverage, explained_codes
changelog: `parTraverse` on a store or held `Map` is renamed `parForEach`, matching `List`, and the old spelling is an error with a machine-applicable fix
breaking: `parTraverse` on a `store Map` or a held `Map[K, Connection]` (the query form that runs a step per element concurrently and returns `()`) is now `parForEach`; the old spelling is the error `bynk.query.par_traverse_renamed`, and `bynk check --fix` renames the call.
---

## ADR: query-par-for-each
title: Query.parTraverse becomes parForEach, matching List
summary: Renames the concurrent unit-returning step on a store or held Map query to parForEach, with an error and a fix for the old name

**Context.** #3967 renamed `List.parTraverse` (which returned `()`) to `parForEach` and gave `parTraverse` the meaning "returns the values". The `Query` vocabulary, used on store `Map`s and held `Map[K, Connection]`s, still spelt its concurrent unit-returning step `parTraverse`, so the same name meant "returns values" on a `List` and "returns `()`" on a query, which broke the grid #3967 set out to make consistent.

**Decision.** `Query.parTraverse` becomes `parForEach`, with the same semantics and closure rules (`bynk.par.store_write_in_closure` applies as before). The old spelling is the error `bynk.query.par_traverse_renamed`, carrying a machine-applicable fix, so `bynk check --fix` migrates it; unlike the `List` form, it cannot still type-check under a new meaning, so it is an error rather than a warning. No value-returning `Query.parTraverse` is added: the existing uses are fire-and-forget broadcasts over connections and counters, so it is deferred until a use appears (`collect()` the query, then `parTraverse` the list, covers it meanwhile). The `limit:` option of #3968 is not offered on the query forms. In the compiler the retired spelling stays a hidden `QueryOp`, dispatched so the checker can report the rename and keep checking the call as `parForEach`, but it is not offered by completion and never reaches lowering.

**Consequences.** Breaking for every program that calls `parTraverse` on a store or held `Map`; it lands before 1.0. Repository fixtures, the book and the reference are migrated in the same change. Hover and completion offer `parForEach` on queries; semantic tokens and signature help are unchanged.
