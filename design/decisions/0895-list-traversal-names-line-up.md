# 0895 — parForEach and a value-returning parTraverse complete the traversal grid

- **Status:** Accepted (v0.646)

**Context.** With `traverse` (#3962) the sequential side had `forEach` (returns
`()`) and `traverse` (returns values), but concurrently `parTraverse` returned
`()`, so one name meant "values" in order and "nothing" concurrently, and there
was no way to run steps concurrently and keep what they return.

**Decision.** `List.parTraverse` becomes `parForEach` (same semantics, same
closure rules), and a new `parTraverse(f: T -> Effect[U]) -> Effect[List[U]]`
runs the steps concurrently and returns the values in input order. Faults follow
`par` and `parTraverseAll`: wait for every step to settle, then raise the first
fault in input order. The store-write closure rule
(`bynk.par.store_write_in_closure`) applies to `parForEach` and the new
`parTraverse`. Migration is in one release: a unit-returning `parTraverse` still
type-checks (it is `Effect[List[()]]`), and `bynk.list.par_traverse_unit`
(warning, machine-applicable) suggests `parForEach`; the fix is attached to the
warning rather than to the downstream type error, so `bynk check --fix` migrates
a call whose result is used as `Effect[()]` as well as one that is discarded.
`parTraverse` on a `store` query or a held `Map` (the `Query` vocabulary) keeps
its meaning; the same rename there is a separate question.

**Consequences.** The warning fires whenever the step returns `()`, including a program that genuinely uses the `List[()]` result (`let rs <- xs.parTraverse(f)` then `rs.length()`); `bynk check --fix` would rename that call to `parForEach` and turn it into a type error. That case is rare, and one-step migration of the common one is the trade accepted here; such a call can keep its meaning by giving the step a non-unit result. The new `parTraverse` lowers to the existing settle-all join,
as `parTraverseAll` does, so no runtime change. Existing `List.parTraverse` call
sites are migrated in the same change.
