---
level: minor
probes: emit_diagnostics, diagnostic_coverage
changelog: "**Breaking:** a generic that compares a type parameter now declares it, `fn same[T: Eq](a: T, b: T)`, or `where A: Eq` for a method's receiver type parameter, in place of ADR 0430's inferred bound (constrained-generics slice 0); `bynk.generics.missing_eq_bound` carries a machine-applicable fix, and `bynk.generics.no_bounds` is retired; see [the migration note](/book/troubleshooting/generics-missing-eq-bound/)"
---

## ADR: declared-eq-bounds
title: Type-parameter bounds are declared, and `Eq` replaces the inferred equality bound
summary: Bounds `T: Eq` and a method `where A: Eq`; `==` on an undeclared type parameter is `missing_eq_bound`; amends ADR 0430

**Context.** ADR 0028 left generics without bounds, and ADR 0430 closed the gap
for equality by *inferring* which type parameters a generic compares, then
checking each call against that inferred set. The constrained-generics track
(`design/tracks/constrained-generics.md`, Q3 (b) and Q8) adopted declared bounds
instead, because the other constraints (`Ord`, `Key`, `Codable`) will have to be
declared, a requirement a caller must meet should be readable from the callee's
signature, and the language should not keep two spellings of one requirement.
This is slice 0, proposed in #2639. It amends ADR 0430: its checking at the call
site stands, but the bound is declared instead of inferred.

**Decision.**
1. **Bound syntax.** A type parameter takes bounds inline: `T: Eq`, and
   `T: C1 + C2` for several. The grammar accepts any constraint name, and the
   checker admits only `Eq`: any other name is `bynk.generics.unknown_constraint`.
   The `+` form parses now so later slices add constraints without a grammar
   change. `bynk.generics.no_bounds` is retired. A bound parses on `fn`, `type`
   and capability-operation type parameters.
2. **Receiver type parameters.** A method that compares its receiver type's
   parameter (`A` in `Box[A]`), which it does not declare, bounds it with a
   method-level clause after the return type and before any contract clause:
   `fn Box.has(self, x: A) -> Bool where A: Eq`. Only callers of `has` are
   constrained, so `Box[(Int) -> Int]` stays legal where nothing compares it.
   A `where` entry names a type parameter of the method or of its receiver type,
   otherwise `bynk.generics.unknown_where_param`.
3. **Bounds on type declarations.** `type Set2[T: Eq] = …` is legal and is checked
   where the type is applied to a written type argument (`Set2[(Int) -> Int]` is
   rejected with the equality diagnostics). Nothing requires one in slice 0, so
   a bare type-parameter argument is not checked, and neither is an instantiation
   inferred from a construction; later slices that require a bound close both.
   A bound a receiver type declares holds in its methods.
4. **No bounds on capability operations.** A generic capability operation has no
   body for a bound to serve; `bynk.generics.bound_on_capability_op` until the
   `Codable` slice gives one a use.
5. **Hard cutover.** Inside a body, `==`/`!=` on a type that mentions a type
   parameter requires that parameter to be declared `Eq` (directly, or by the
   method's `where` clause, or by its receiver type's declared bound), otherwise
   `bynk.generics.missing_eq_bound` at the comparison. At a call, each `Eq`-bounded
   type parameter of the callee must be instantiated with an equality-supporting
   type (the existing `not_comparable`, `stream_not_comparable` and
   `held_not_comparable` diagnostics, unchanged), and a type parameter the caller
   passes must itself be declared `Eq`, otherwise `missing_eq_bound` at the call.
   Every `missing_eq_bound` carries one machine-applicable suggestion (insert
   `: Eq`, ` + Eq` after an existing bound, or the `where` clause, as the
   parameter is the function's own or its receiver's), reported once per
   parameter per function. The editor's quick fix and `source.fixAll.bynk` apply
   it with no new wiring. Pre-1.0 an increment may break source, and a
   one-release warning would leave two accepted spellings.
6. **The inference is kept to compute the fixes.** ADR 0430 computed each
   generic's compared parameters by re-checking its body into throwaway sinks,
   cached per declaration across units. That scan no longer decides what a call
   accepts: a call is checked against the callee's *declared* bounds. It is
   consulted only to hold a call to a generic that compares a parameter it has
   not yet declared to that parameter too, so one pass reports every link of a
   chain (`twice` calling `same`, across units) and one fix-all migrates the
   lot, instead of each round exposing the next caller. The `missing_eq_bound`
   at the callee's own body is reported either way, so the scan changes no
   program's accept/reject outcome.
7. **Bounds are part of the signature.** A cross-unit signature carries each
   parameter's bounds and the `where` entries, so a changed bound changes the
   contract hash; a signature with no bounds hashes as before. Bounds erase:
   nothing is emitted for them, `==` on `T` still lowers to `__bynkEq`, and the
   emitted TypeScript is byte-identical to before.

**Consequences.**
- Every generic that compared a type parameter must add `: Eq` (or `where A: Eq`)
  before it compiles; the diagnostic's fix does it. The in-repo corpus was
  migrated by applying those fixes. A CLI `bynk check --fix` is a separate
  increment (#2639 DECISION F).
- Calls make the same accept/reject decisions as under ADR 0430: the fix is
  exactly the parameters the inference found, so applying it to an ADR 0430
  fixture preserves every call site's outcome.
- The constraint table has one entry, `Eq`; later slices add `Ord`, `Key` and
  `Codable` there, and add the constraints' run-time witnesses where one is
  needed.
- Editor surface (ADR 0156): hover, signature help and completion detail show a
  generic's bounds and `where` clause; completion offers the constraint names
  after `T:`, `+` and `where A:`; constraint names are `interface` semantic
  tokens (appended to the frozen legend); the formatter preserves bounds.

Proved by:
- positive `2629_eq_bounds_declared`; negatives `2630`–`2636` (missing bound in
  a body, at a call, on a receiver; unknown constraint; bound on a capability
  op; a type-declaration bound at instantiation; an unknown `where` parameter);
- the migrated ADR 0430 fixtures (`1688_*`, `1652_structural_equality`), and
  `bynkc/tests/eq_bound_migration.rs`, which applies each suggested fix to the
  pre-migration sources and asserts the call-site outcome is unchanged;
- the tree-sitter grammar, its corpus and the cross-parser conformance cases.
