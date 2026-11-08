---
level: minor
changelog: An `Oidc` route is tested at the `system` tier. The driver is the issuer, and `by Forged`, `by Expired`, `by WrongIssuer` and `by WrongAudience` drive its seam
probes: diagnostic_coverage
---

## ADR: oidc-system-tier-test-issuer
title: The system driver is the issuer for an `Oidc` route
summary: A test-process ES256 issuer answers the Worker's JWKS fetch; four failure principals drive an `Oidc` seam

**Context.** A `system` case could not call a route guarded by an `Oidc` actor
(`bynk.test.oidc_needs_in_process`): the driver held no key pair for the JWKS, so
the request reached the real seam with no token. The seam's refusals (a bad
signature, another issuer or audience, an expired token) were therefore guarded
only by the compiler's own Rust-driven suite, and an author could test the claim
logic only in process. #2864 gave `Bearer` routes `by Forged` and `by Expired`
and deferred `Oidc` for want of a test key pair (proposed in #3052).

**Decision.**

- **The driver is the issuer, in the test process.** The generated test module
  holds one ES256 key pair per process (a second, never-published pair signs
  `by Forged`), a fixed `kid`, and a wrapper over `globalThis.fetch` that
  answers a `GET` of the actor's resolved `jwks` URL with the test JWKS and
  passes every other request through. `system` imports the Worker into the test
  process, so the unmodified seam's own JWKS `fetch` reaches the wrapper.
  Nothing is emitted into a Worker; no flag, environment variable or alternative
  trust path exists in production code.
- **One issuer per process, with a fixed `kid`,** because the seam's JWKS cache
  is keyed by URL with a ten-minute TTL: a key regenerated per suite would be
  refused from the cache. `by Forged` keeps the real `kid`, so the seam finds a
  candidate key and refuses on the signature, the path that matters.
- **Four failure principals.** `by Forged` (signed by the unpublished key),
  `by Expired` (`exp` an hour past), and the `Oidc`-only `by WrongIssuer` and
  `by WrongAudience` (correctly signed, unexpired, `iss` or `aud` not the
  actor's). Each carries a valid `sub`, so only one thing is wrong. `by Nobody`
  widens to `Oidc` routes. `by Workload("wid", TokenClaims.of([…]))` mints a
  valid token whose `sub` is the identity and whose claims are the string lines.
- **Trust roots resolve as the seam resolves them.** The declaration, replaced
  by the actor's `trustFrom` variable's JSON when the case's `env` sets it, so
  the minted token always matches what the seam checks.
- **Claims must be a literal `TokenClaims.of([…])` at `system`**
  (`bynk.test.system_claims_need_literal`): a `TokenClaims` is a view with
  `get`/`has`, not its lines, and exposing them would change the runtime every
  Worker embeds.
- **Remote targets.** A deployed Worker fetches its own JWKS, which the driver
  cannot answer, so an `Oidc` call against a remote target fails at run time with
  a message naming the cause, rather than a `401` that would pass against a real
  issuer.
- **Retired.** `bynk.test.oidc_needs_in_process` has no remaining case. Added:
  `bynk.test.wrong_issuer_needs_oidc_route`,
  `bynk.test.wrong_audience_needs_oidc_route`,
  `bynk.test.system_claims_need_literal`.

**Consequences.** Every scheme's seam is reachable at `system`. The production
Worker is byte-identical; only generated test modules change. A mutation test
turns each check of the emitted `Oidc` verifier off in turn and expects exactly
the matching refusal case to fail. The fetch wrapper is installed once and lives
for the test process (not restored per suite), which is harmless because the
process exists only to run the tests and the wrapper answers only the registered
JWKS URLs. RS256 verification is still covered by the compiler's own
`oidc_auth.rs`, not by the driver.
