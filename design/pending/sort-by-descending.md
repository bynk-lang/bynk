---
level: minor
probes: ts_writes
changelog: `List` and `Query` gain `sortByDescending`: the exact reverse of `sortBy`'s order, stable on ties (#2805)
---

## ADR: sort-by-descending
title: sortByDescending on List and Query
summary: Stable exact-reverse sort over any Ord key; discharges ADR 0116's deferral

**Context.** ADR 0116 D2 left `sortByDescending` "deferred, not dropped" and said descending is `sortBy` then a reverse. That reverse is not equivalent: it reverses equal keys, breaking the stability `sortBy` has. Probes worked around it with `0 - x` keys. The key set it waited on is now open: any `Ord` type is a key on both `List` and `Query` (ADR 0681, #2787).

**Decision.** `sortByDescending(key: T -> K)` on `List[T]` and `Query[T]`, with `sortBy`'s key rules (`bynk.types.key_not_orderable`). The maintainer chose (2026-11-02) both receivers and a **stable** sort: equal keys keep input order.

- *`NaN` placement.* The maintainer first chose `NaN` last in both directions, then revised it the same day to **`NaN` first**: descending is the exact reverse of the ascending order (ADR 0681: `NaN` greatest). The revision is because a generic `T: Ord` key is an erased comparator witness that cannot honour a `NaN`-last rule, so a generic helper would sort differently from the same code inlined. The exact reverse behaves identically for concrete, `derives Ord` and type-parameter keys, and matches PostgreSQL's `DESC` default.
- Lowering: `Array.prototype.sort` is stable, so descending is one stable sort with the key comparator's arguments swapped (a bare-operator swap for `Int`/`String`/`Duration`/`Instant`), on a `List` and inside the lazy `Query` thunk alike. No runtime change.
- The name is `sortByDescending`, as ADR 0116 reserved. Multi-key sorts and custom comparators stay deferred.

**Consequences.** Checker: one kernel arm shared with `sortBy` on each receiver, and the registry entries (`kernel_methods.rs`) that drive hover, completion and signature help. Emitter: the swapped sort on both receivers. No grammar, wire, runtime or ABI change. Fixtures pin ties, `NaN` first (bare, record and generic keys) and a store `Query`.
