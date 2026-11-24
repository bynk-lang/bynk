# 0924 — A type an adapter exports transparent is constructible by its consumers

- **Status:** Accepted (v0.667)

**Context.** Spec type-system §6.5 refused `.of` and a variant constructor from outside a context-owned type (`bynk.context.external_construction`), whatever the export visibility. That made the computed form of a standard-library refined type, `KvTtl.of(d)`, unreachable for a consumer, and was inconsistent with the record literal a transparent export already allows.

**Decision.** The maintainer ruled that a type exported `transparent` is constructible from outside, an opaque one is not, and `.unsafe` stays owner-only (it is an unchecked assertion, not a construction; decided). This change implements it for types an **adapter** owns: `.of` on a refined type, a variant of a sum and record syntax. For a type a *context* owns, the rule is unchanged for now: under the Workers target a consumer imports a consumed context's types type-only (#661, the callee's module stays out of the caller's bundle), and the callee does not export a constructor to import, so opening `.of` there would emit TypeScript that does not compile. Extending the ruling to contexts needs the emitter to give a consumer its own constructor (as it does for codecs); that is #4213.

**Consequences.** Non-breaking: it only accepts more. `ConsumedType` records whether its owner is an adapter. The `.of` of `KvTtl` and `KvCacheTtl`, `After(ttl)` and `KvPut { … }` from a consumer are covered by the `2590_kv_complete_*` fixtures.
