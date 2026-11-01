---
level: minor
changelog: "`parTraverse`, `parTraverseAll` and `parTraverseTry` (on lists and on query/broadcast receivers) now settle all: when an element faults the call waits for every element to settle, then raises the first fault in input order, instead of the first fault to arrive with its siblings left running; no source change (concurrent effects, slice 0b, [#2662](https://github.com/bynk-lang/bynk/issues/2662))"
---

## ADR: par-traverse-settle-all
title: The `parTraverse` family joins settle-all, raising the first fault in input order
summary: parTraverse, parTraverseAll and parTraverseTry, on lists and on query/broadcast receivers, lower to `await __bynkSettleAll(xs.map((x) => () => f(x)))` instead of `Promise.all`, so a fault waits for every element to settle and the first fault in input order is raised

**Context.** Slice 0 of the concurrent-effects track (spine #2615, ADR for the
`par` block) adopted settle-all fault semantics for joins (Q3). The `parTraverse`
family kept `Promise.all`, which raises the first rejection to arrive and leaves
its siblings running unobserved. That breaks the track's constraints
"deterministic results" and "no work outlives the handler unseen", and leaves two
forms with different fault semantics for one idea. ADR 0135 (D1) recorded the
`Promise.all` limit of the broadcast `parTraverse` and named `Promise.allSettled`
as the follow-on; ADRs 0171–0174 specified the list and query forms with "each
runs to completion" and no ordering for faults.

**Decision.**

- **A. Lowering.** Reuse `__bynkSettleAll` over per-element thunks:
  `await __bynkSettleAll(xs.map((x) => () => f(x)))`. Building the thunk array
  calls nothing; the helper then issues the effects in input order, and a
  synchronous throw from `f` is a rejected entry rather than something that
  orphans the elements already issued. **Rejected:** a dedicated
  `__bynkSettleMap(xs, f)` (a second helper with the same contract, saving one
  closure per element, which is noise next to a subrequest).
- **B. Coverage.** All three operations, on lists and on query/broadcast
  receivers, in one increment. Sequential `forEach`, `traverseAll` and
  `traverseTry` are untouched. Each lowering notes the runtime import, so a
  module using the family but no `par` block imports it; every other module emits
  byte-identically.
- **C. Fault reporting.** Identical to `par`: wait for every element, then raise
  the first fault in input order; later faults ride on it as `suppressed` and are
  not catchable separately (ADR 0444).
- **D. `parTraverseTry` precedence unchanged.** A fault anywhere wins over any
  `Err`; `Err`s are scanned in input order only when every element succeeded.
- **E. Observation unchanged.** Issue order is input order before and after, so
  `expect A.op before B.op` behaves as it did.
- **F. Compatibility.** A behaviour change, no migration. Programs whose elements
  never fault are unaffected. A faulting program now raises a deterministic fault
  after its slowest sibling settles rather than at the first rejection to arrive.
  This does **not** give failure isolation (a dead connection still raises, after
  the rest have settled); isolation stays `parTraverseAll`'s job.

**Consequences.** A fast fault waits for a slow sibling: the price of determinism
and of keeping work inside the handler, the same trade `par` makes. This amends
ADR 0135 (D1) and ADRs 0171–0174 on fault semantics only; their signatures,
issue order and `Result` handling stand. No grammar, checker, runtime-library or
tooling change.
