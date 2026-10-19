---
level: minor
probes: emit_diagnostics, ts_writes, diagnostic_coverage
changelog: On an actor-sum route, the `system` driver now sends the credential of the member the case acts as (a token for a Bearer member, a signature for a `Signature` member, nothing for `Visitor`), instead of the primary member's. A `system` case can now call a sum route as any member, and `by Nobody`, `by Forged` and `by Stale` work on a sum with a `Signature` member. `bynk.test.signature_needs_in_process` is retired, and a forged signature is now made with a secret derived from the real one (#2155)
---

## ADR: system-tier-sum-route-credential
title: The `system` driver picks an actor sum's credential from the call-site principal
summary: Per-call credentials on sum routes retire the last `signature_needs_in_process` case

**Context.** The `system` driver chose a route's credential from its `by`
clause's primary member. On an actor sum that was wrong whenever the case acted
as another member: on `by who: Processor | User` (a `Signature` primary), `by
User("bob")` passed check but went out with no token and was refused with a
`401`; on `by who: User | Visitor`, `by Visitor` carried a token and resolved as
`User`. The slice 2 signer (#2135) therefore did not sign sum routes, and kept
`bynk.test.signature_needs_in_process` for a call to a sum as its `Signature`
member (#2155).

**Decision.**

- **Per call.** An actor-sum route's signing drivers take the actor the call
  acts as: the call-site principal, else the primary member. They build that
  member's credential: a signed token for a Bearer member, a signature over the
  exact bytes sent for a `Signature` member, nothing for a member with no
  scheme. A single-actor route is unchanged.
- **Failure principals on a sum.** `by Nobody` is legal on a sum with a Bearer
  or `Signature` member. `by Forged` and `by Stale` are legal on a sum with a
  `Signature` member, and sign as its first `Signature` member. None of the
  three is admitted on a sum with an open member (`Visitor`, or an actor with
  no auth scheme): its seam resolves a missing or bad credential as that
  member, so it never answers `401`.
- **Retired.** `bynk.test.signature_needs_in_process` had no remaining case and
  is removed.
- **The forged secret** is the real one with `!` appended, so it differs from
  whatever the seam holds (an `env` entry or the fixed test secret), rather than
  a fixed literal.

**Consequences.** Every member of an actor sum is reachable at `system`, as
itself. The `Oidc` refusal (`bynk.test.oidc_needs_in_process`) stays: there is
still no `Oidc` token issuer.
