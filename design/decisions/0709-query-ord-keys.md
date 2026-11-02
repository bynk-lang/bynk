# 0709 — A Query's sortBy, min and max keys are Ord

- **Status:** Accepted (v0.487)

**Context.** ADR 0681 decision 8 left a `Query`'s `sortBy`/`min`/`max` on the five-base-type rule, reasoning that storage ordering has no SQL lowering for a user-type comparator. Every `Query` operation is in-memory array code (`lower_query_method`), and nothing in the roadmap lowers one to a query language. Proposal #2787.

**Decision.**
1. **Widen to `Ord` (A).** `Query.sortBy`/`min`/`max` call the same `require_key_ord` as `List`; `is_orderable` and `require_orderable` are removed. A future pushdown backend owns its own refusal of non-base keys.
2. **`Float` takes the total order (B).** A `Float` key uses `__bynkCmpFloat` (`NaN` last, `-0 == 0`), as on a `List` (ADR 0681 G). It changes emission, and behaviour only when a key is `NaN`.
3. **Opaque keys follow the `Ord` rule (C).** An opaque type is sortable only where it is `Ord` (`derives Ord`, or inside its defining commons).
4. **Emission (D).** `QueryShape` carries the comparator from `kernel_key_comparator`, as `lower_list_kernel` does; the arm binds it with `with_comparator`. Base keys other than `Float` keep the bare operators.
5. **Generics (E).** A `T: Ord` key resolves through the existing witnesses; nothing new is designed.

**Consequences.** A store query orders by a `derives Ord` enum or record, `Option`, `List`, `Bytes` or a `T: Ord` key before `skip`/`take`. `Query.min`/`max` return the key.
