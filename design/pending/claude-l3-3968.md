---
level: minor
probes: emit_diagnostics, diagnostic_coverage, explained_codes
changelog: `parForEachWith`, `parTraverseWith`, `parTraverseAllWith` and `parTraverseTryWith` bound a concurrent fan-out with a `ParOptions { limit }` from the new `bynk.par` commons
---

## ADR: par-fan-out-options-record
title: A bounded fan-out is spelled with a ParOptions record, not a named argument
summary: Adds the four ...With fan-out forms taking ParOptions { limit } so concurrency can be capped without a grammar change

**Context.** `parForEach`, `parTraverse`, `parTraverseAll` and `parTraverseTry` start every step at once, which exhausts connection and subrequest limits on a long list. A bound is needed, but calls are positional only (ADR 0780) and there are no optional parameters or overloading (ADR 0051), so a `limit:` argument or a defaulted parameter would need a grammar change.

**Decision.** Four new `List` methods, `parForEachWith`, `parTraverseWith`, `parTraverseAllWith` and `parTraverseTryWith`, each take `(f, options: ParOptions)`; the plain forms are unchanged. `ParOptions` is `{ limit: Int }` with no default, declared in the new first-party `bynk.par` commons and brought in with `uses bynk.par` (a commons rather than the prelude, so nothing is added to every program's namespace). `limit` is a whole number from 1 to 1000. A literal or constant outside that range is the error `bynk.list.par_limit_range`; a limit of 1 is the warning `bynk.list.par_limit_one` (use the sequential form); a limit computed at run time is checked by the runtime, which faults the call. Steps run on a sliding pool, so a slot is reused as soon as any step settles. Faults are settle-all as for the plain forms: every started step settles, the first fault in input order is raised and the rest ride on it as `suppressed`. `parTraverseTryWith` starts no further step once one returns `Err`. The store-write closure rule (`bynk.par.store_write_in_closure`) applies to all four. General named arguments are proposal #4194, which would give the future spelling `limit:`; nothing of it is implemented here.

**Consequences.** Additive: no existing program changes. When named arguments land, the `…With` forms can gain a `limit:` spelling and the record form can be deprecated. The pool is one runtime helper (`__bynkSettlePool`) shared by all four forms.
