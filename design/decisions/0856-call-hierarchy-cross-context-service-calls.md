# 0856 — Service handlers are call-only index symbols, so call hierarchy covers cross-context calls

- **Status:** Accepted (v0.626.1)

**Context.** Call hierarchy (ADR 0067, 0069) covered every call relation but the one that matters most for architecture: a call from one context into another's service. It was recorded only as a `Service` reference, and service handlers had no index symbol to be a callee (#3582).

**Decision.** A service's `on call` handler and each `on http` route become `SymbolKind::Handler` symbols keyed `(unit, "service.call")` and `(unit, "service.POST /orders")`. A cross-context call records a `Handler` reference to the resolved handler alongside the existing `Service` reference, through plain and aliased `consumes`, and the pair becomes a `CallEdge`. Call hierarchy names an HTTP route by method and path, as the sequence and wire-contract models do, with the unit as `detail`.

The symbols are *call-only* (`SymbolEntry::call_only`): the definition is the handler's `on` keyword, and the symbol is skipped by semantic tokens, code lenses, workspace symbols and rename, and its call-site refs are ignored by `symbol_at`, so Find References on a service, hover and token colouring of a service call are unchanged.

**Consequences.** The hierarchy's caller granularity is unchanged: the caller is the calling owner (a service or agent), not its individual handler. Cron, message and other handler kinds stay unindexed.
