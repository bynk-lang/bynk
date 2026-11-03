---
level: minor
probes: emit_diagnostics, ts_writes, diagnostic_coverage
changelog: "`by Forged` and the new `by Expired` drive a `Bearer` route's seam at `system` with a wrong-secret or an expired token, each read as `Rejected(Unauthorized)` ([#2864](https://github.com/bynk-lang/bynk/issues/2864))"
---

## ADR: bearer-forged-expired-principals
title: by Forged and by Expired drive a Bearer route's seam
summary: Reserved failure principals for Bearer routes at system, reusing Forged and adding Expired

**Context.** A `Bearer` actor's main claims are that a token signed with another secret is refused and that an expired token is refused (`exp`/`nbf`, ADR 0085). Neither could be tested in Bynk: `by Forged` and `by Stale` were gated to `Signature` routes (#2135, ADR 0525), and `by Forged` on a Bearer route was `bynk.test.forged_needs_signature_route`. The system driver already mints Bearer tokens for `by User("ada")` (ADR 0207 D3), so the signer was there.

**Decision.**

- **A. `by Forged` widens to Bearer routes.** Its meaning, a credential made with a secret the seam does not hold, is scheme-independent. On a Bearer route the driver sends a well-formed token with a valid `sub` and a future `exp`, signed with a secret derived from the real one (`secret + "!"`), so only the signature is wrong. `bynk.test.forged_needs_signature_route` now reads "needs a `Signature` or `Bearer` route" for `Forged`; `Stale` stays `Signature`-only. On a sum with no `Signature` member the token is minted for its first Bearer member; with one, `Forged` keeps signing as that member.
- **B. A new `by Expired`**, not a reuse of `by Stale`: `Stale` is a stale signed timestamp, an expired JWT is a different claim. The driver sends a correctly signed token with a valid `sub` and an `exp` one hour before the test clock. `bynk.test.expired_needs_bearer_route` refuses it off a Bearer route. `by Premature` (`nbf` in the future) is deferred.
- **C. `system` tier only**, as for `Forged` and `Stale`: below it the verifier is skipped and the identity is given. Both are refused at `unit` and `integration` with the existing `bynk.test.forged_needs_system`.
- **D. `Oidc` is out of scope.** An `Oidc` route would need a forged or expired token against a test JWKS, which needs a test key pair; that is a separate slice.

The drivers mint the token with the existing test signer, `__bynkSignHs256(payload, secret)`, which already takes the `exp` and the secret as arguments, so no new signer entry point is needed. The `sub` is the fixed string `bynk-test`, not the call's identity (`by Forged` names none), so it satisfies a `NonEmpty`-style identity refinement and the seam refuses on the signature or `exp`, not on the `sub`. The response decodes through the unauthenticated outcome (ADR 0212): a `401` reads as `Rejected(Unauthorized)`.

**Consequences.** A `Bearer` actor's refusal claims live in the suite. A principal that could pass if the seam were broken is a credential the seam must refuse, so a `Handled(_)` outcome is a reported failure; the `2864_system_bearer_refusals` behavioural fixture asserts the outcome is observed. The production Worker and its verifier are untouched. The tooling follows: hover for `Expired` and for `Forged` on Bearer, and `Expired` completes after `by` in a suite. The `exp` margin of an hour keeps the cases independent of clock skew.
