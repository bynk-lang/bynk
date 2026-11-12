---
level: minor
changelog: A Worker that calls `Idempotency.claim` declares an implicit D1 database for its claims, which `bynk deploy` and `bynk dev` provision with no author action
probes: emit_abi_shapes, ts_writes
---

## ADR: platform-binding-of-a-portable-capability
title: A platform's binding of a portable capability may be durable; it is not a selectable provider
summary: bynk.Idempotency's Cloudflare binding stores claims in D1, chosen by the build's platform, within ADR 0016

**Context.** ADR 0016 dropped a portable infrastructure tier with selectable platform providers and a foreign-capability provision rule. #2840 wants `Idempotency.claim` to be durable on Cloudflare, which looked like it needed a way for a platform unit to supply a durable provider for a portable capability.

**Decision.** It does not. The first-party `bynk` unit already has one binding module per platform (`bynk-cloudflare.ts`, `bynk-node.ts`, `bynk-browser.ts`), each with its own `IdempotencyProvider`, and a first-party provider can already be handed the Worker `env` (`env_providers`). The Cloudflare binding's `claim` stores claims in the implicit D1 database the compiler declares for a Worker that calls it; the Node and browser bindings stay in memory. The platform is chosen by the build, as for every other `bynk` capability (`Fetch`, `Logger`, `Locale`), so there is no selectable-provider mechanism and no rule letting a user provide a capability declared elsewhere, which stay refused (`bynk.provider.unknown_capability`). A durable provider remains something a first-party platform binding supplies, never something an author selects.

**Consequences.** ADR 0016 stands unchanged; this records that a platform's binding of a portable capability may be durable (it is no "lowest common denominator" in storage) provided the capability's semantics hold on every platform: `claim` is first-writer-wins everywhere, durable only where the platform gives it a store. The document of a capability states what each platform's binding guarantees. `dedup`/`remember` stay per-instance until typed capability reads (#3458) land.
