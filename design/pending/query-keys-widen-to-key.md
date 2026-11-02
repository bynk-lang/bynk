---
level: minor
probes: ts_writes
changelog: A `Query`'s `distinct`, `distinctBy`, `groupBy` and join keys take any `Key` type, as a `List`'s do
---

## ADR: query-keys-widen-to-key
title: A Query's distinct, groupBy and join keys are Key
summary: Query shares List's Key rule and keyed helpers; stores keep String/Int

**Context.** ADR 0690 decision 9 kept a `Query`'s `distinct`, `distinctBy`, `groupBy`, `joinOn` and `leftJoin` keys on the `String`/`Int` rule, because the `Query` emission hashed keys through `String(key)`. That is an emitter detail: `List` already routes a non-primitive key through the keyed helpers in `keys.ts`. Proposal #2789.

**Decision.**
1. **Widen all five together (A).** `require_keyable` (`bynk-check/src/checker/kernels.rs`) sends every kernel, `List` or `Query`, through `check_key`; `is_query_keyable` is removed. A `Float` at any depth, a `Map`/`Set`, a function and the other non-`Key` types are still `bynk.types.not_keyable`.
2. **Emitter shape (B).** `lower_query_method` takes a `QueryShape` (element types plus a `keyed` flag). Each caller computes the flag with `query_keyed`, which mirrors `lower_list_kernel`'s `key_is_plain`/`key_fn_is_plain` choice and notes the `keys` runtime helper.
3. **Plain path unchanged (C).** A primitive-keyed `Query` emits exactly what it did; only `Bytes`, `Decimal`, `Option`, `List`, records, sums and enums take `__bynkDistinct`, `__bynkDistinctBy`, `__bynkGroupBy`, `__bynkJoinOn`, `__bynkLeftJoin`.
4. **Ordering is separate (D).** `Query` `sortBy`/`min`/`max` keep the five-base-type rule (#2787).

**Consequences.** A store query groups, dedups and joins by a record or enum key by value. No runtime change. Store `Map`/`Set`/`Cache` and `@indexed` keys still take only `String`/`Int`.
