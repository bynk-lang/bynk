---
level: patch
changelog: A call to a service whose signature names a `bynk` prelude type (`CloseInfo`, `List[CloseInfo]`, `Cookie`) has the type the service declares, in a `system` case and in a consuming context, instead of `()`
---

## ADR: contract-table-holds-the-adapter-types-a-signature-names
title: A context's contract table includes the first-party adapter types its service signatures name
summary: `contract_types_for` adds, for each service signature, the `bynk` and `bynk.cloudflare` types it names and what they reach

**Context.** #4618: `let c <- shop.orders.one()` in a `system` case, where `one` returns `Effect[CloseInfo]`, failed with `bynk.effect.bind_on_non_effect`, because the call's result type was `()`. A consuming context failed the same way. A caller reads a consumed service's signature in `contract_types_for(callee)`, which holds the callee's own types, its `uses` commons' and the types of the contexts it consumes. The names a callee takes from a first-party **adapter** (`consumes bynk { CloseInfo, Cookie }`) were in none of these, so the signature did not resolve and the checker read the result as `()`.

**Decision.** `contract_types_for` adds the types of the first-party units a unit consumes (`bynk`, `bynk.cloudflare`, `bynk.crypto`; the table records them as `first_party_consumes`) that its service signatures name, directly or through one of its own types, with what those types reach. A type renamed with `consumes U { T as A }` is filed under its alias. A name an adapter type reaches that the unit binds to a declaration of its own (its `SameSite` beside `Cookie`'s) is filed under a qualified key (`bynk__SameSite`) and the reference renamed, as for a consumed context's types (#2087). The unit's own `consumes` record is read, not the `consumes` map a `system` harness passes, which holds only its participants for its target's entry (#2832). That one table is what the callee hashes its contract from and what a caller types a call and hashes its expectation from, so the two sides still agree by construction (§6.5).

**Consequences.** A call to a service returning a prelude type is typed, and a `system` case can read its fields. The contract hash of a service whose signature names a prelude type now includes that type's shape, where it named it unresolved before, so its hash changes once, on both sides of a call. A deployed callee built before this change and a caller built after it disagree on such a service's contract until both are redeployed, which the contract check reports as skew. No service signature that names only the unit's own, commons' or consumed contexts' types is affected.
