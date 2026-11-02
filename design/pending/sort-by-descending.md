---
level: minor
probes: ts_writes
changelog: `List` and `Query` gain `sortByDescending`: stable, largest key first, `NaN` last in both directions (#2805)
---

## ADR: sort-by-descending
title: sortByDescending on List and Query
summary: Stable descending sort over any Ord key, with NaN last; discharges ADR 0116's deferral

**Context.** ADR 0116 D2 left `sortByDescending` "deferred, not dropped" and said descending is `sortBy` then a reverse. That reverse is not equivalent: it reverses equal keys (breaking the stability `sortBy` has) and, with `Float`'s total order (ADR 0681, `NaN` last), moves `NaN` to the front. Probes worked around it with `0 - x` keys. The key set it waited on is now open: any `Ord` type is a key on both `List` and `Query` (ADR 0681, #2787).

**Decision.** `sortByDescending(key: T -> K)` on `List[T]` and `Query[T]`, with `sortBy`'s key rules (`bynk.types.key_not_orderable`). The maintainer chose (2026-11-02) both receivers, a **stable** sort (equal keys keep input order) and `NaN` **last** in both directions.

- *Composite keys.* "NaN last" holds inside a key. The emitter builds the descending comparator from `CmpGen` in a descending mode: `Float` leaves become `__bynkCmpFloatNanFirst` and the whole is wrapped in `__bynkCmpDesc`, which swaps the arguments, so everything else reverses (`None` last, longer list first, later variant first) while `NaN` stays last. A plain `Int`/`String`/`Duration`/`Instant` key swaps the bare operators and needs no comparator import.
- *Type-parameter keys.* A generic `T: Ord` key is an erased witness, so it is reversed exactly; a `Float` instantiation therefore sorts `NaN` first. Documented, not rejected, to keep `sortByDescending` symmetrical with `sortBy`.
- The name is `sortByDescending`, as ADR 0116 reserved. Multi-key sorts and custom comparators stay deferred.
- Lowering: `Array.prototype.sort` is stable, so descending is one stable sort with the descending comparator, on a `List` and inside the lazy `Query` thunk alike.

**Consequences.** Checker: one kernel arm shared with `sortBy` on each receiver, and the registry entries (`kernel_methods.rs`) that drive hover, completion and signature help. Runtime: two small comparators in `compare.ts`. No grammar, wire or ABI change. Fixtures pin ties, `NaN`, a `derives Ord` record key and a store `Query`. The type-parameter-`Float` case is the one place "NaN last" does not hold.
