---
level: minor
changelog: A service handler's `Idempotency.claim` is released when the handler fails and held for its full window when it completes, under a 30 s lease
probes: emit_abi_shapes, ts_writes
breaking: A `claim` in a service handler that fails (a fault, a returned `Err`, or a 5xx `HttpResult`) is now released, so a retry can claim again; before this change a claim was held for its whole window whatever the outcome.
---

## ADR: claim-release-and-lease
title: A handler's idempotency claim is released on failure and leased until it settles
summary: The compiler settles a service handler's claims on exit: release on failure, confirm otherwise, with a 30 s lease for an instance that dies

**Context.** `Idempotency.claim` (#2840) is first-writer-wins. With no release, a handler that fails after `Claimed` leaves the key claimed, so the retry that delivery systems send is told `AlreadyClaimed` and the work never completes. The maintainer decided on #2840 that a claim is released automatically when the handler fails, with no new author-facing surface, and that every claim carries a lease for when the instance dies before it can release.

**Decision.** A service handler that calls the first-party `Idempotency.claim` is wrapped by the emitter. Each claim it makes goes through the handler's claim scope, which the provider supplies through a hidden `__track()` method (not part of the `Idempotency` capability, so authors see no new operation). The claim is taken for `min(expiresAfter, 30 s)`, the lease. When the handler exits the scope settles its claims: a thrown fault, a returned `Err`, or an `HttpResult` that is a 5xx releases them; any other outcome, a 4xx included, confirms them for the full `expiresAfter`. A provider with no `__track` (a user stub) gets a no-op scope. A claim outside a service handler, such as in an agent handler, is a plain call held for its full window. The guarantee is at-most-once completion, not exactly-once side effects: work done before a failure can repeat on a retry, and a handler that runs past the lease can be re-claimed by a concurrent retry.

**Consequences.** The lease is a fixed 30 seconds, matching the `waitUntil` limit; making it configurable needs its own proposal. Agent handlers are not settled yet. The test double and all three in-memory providers implement `__track`, so a test sees the release and the lease on the harness clock. A provider that supplies no scope (a user-written one) gets a no-op scope and takes each claim for its full `expiresAfter`, so no claim is cut to the lease without a settle to extend it; a later durable provider supplies `__track` too. A scope settles a claim only while it is still the handler's own, so a handler that outlives its lease cannot release or overwrite a retry's claim.
