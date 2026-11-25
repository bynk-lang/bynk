---
level: minor
probes: emit_diagnostics, diagnostic_coverage, explained_codes
changelog: A `Bearer` binder, and a refinement of one, reads its verified token's claims with `.claims`, as an `Oidc` binder does, and a test or `system` case supplies them as the principal's `TokenClaims.of([…])`
---

## ADR: bearer-claims
title: A `Bearer` binder reads the verified token's claims with `.claims`
summary: `.claims` on a `Bearer` binder and its refinements, in `TokenClaims`'s own rules, and what is left for sums

**Context.** An `Oidc` binder has `.claims` (`get(name)`, `has(name)`), but a `Bearer` binder did not: a
handler behind an ordinary HS256 JWT could not read a tenant, a scope or a plan claim, though its
refinement predicates (`hasClaim`, `claimEquals`) read the same claims at the boundary. The token is
verified by generated code and the handler never sees it, so the claims can reach the handler only if
the seam forwards them. The `Bearer` verifier already returned them; the seam dropped them (#3937).

**Decision.** A. The same `TokenClaims`, the same two methods and the same boundary rule
(`bynk.types.claims_at_boundary`) as `Oidc`: no new type. B. A refinement of a `Bearer` actor carries its
base's claims (`by a: Admin` has `a.claims`): the claims are the token's, so the binder of the
refinement reads what its predicate was checked against. C. The binder of a sum is the resolved arm, which
today binds the identity value and has nowhere to hang `.claims`; a sum arm reading claims is not part
of this change (see below). D. A test supplies claims as the principal's second argument for an `Oidc` or
a `Bearer` actor, `by User("ada", TokenClaims.of([…]))`; at `system` the driver signs a token whose
`sub` is the identity and whose claims are those lines, and for a refinement beside the witness claims its
predicate needs, which the principal's own override. `bynk.test.principal_claims_not_oidc` keeps its code and
now refuses an actor of any other scheme. The checker's `claims_binder` (was `oidc_binder`) is set for an
HTTP handler with one `Oidc` or `Bearer` binder; the Worker wrapper passes `deps.claims` as the `Oidc` one
does; the editor offers `claims` after a single-actor binder and describes it on hover.

**Consequences.** **Security-relevant:** the claims are read from the token the seam has already verified
(signature, and for a refinement the predicate), so a handler cannot read a claim of a token that was
refused; but a claim says what the token asserts, not whose data the handler may touch, and the reference's
"Who, not whose" section still applies. No claim is stored: `TokenClaims` stays unable to be a field or to
cross a call, a message or an agent. Where the binder reads no claims (a binder-less `by`, another scheme,
a sum, a handler of another protocol) the emitted `deps` and wrapper are unchanged. A `Bearer` binder's
`deps` type gains a `claims` field only where the handler is HTTP. Reading claims in a sum arm needs a
syntax the issue does not name (a second binder on the arm, or a flow-typed `who.claims`), and is
deferred to #4344, which lays out both options. Until then a route that reads claims names a single
`Bearer` actor, or a refinement of one, in its `by` clause. The test runtime's claim-line helper moves from the `Oidc` issuer to the JWT signer (and
becomes `__bynkClaimLines`), so a target with `Bearer` claims alone does not pull the issuer in.
