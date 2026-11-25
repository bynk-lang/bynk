# 0991 — A `SignedCookie` and an `Oidc` actor may be sum members

- **Status:** Accepted (v0.726)

**Context.** The most common arrangement for an API is a web app signed in with a session cookie plus
CLI, mobile or partner clients sending a token, against the same routes. `SignedCookie` stood alone on its
route and `Oidc` was single-actor only, so a service duplicated every route under two paths, one per scheme.
Both exclusions were recorded as temporary: ADR 0175 D (the sum wrapper tried members synchronously, a
shape the asynchronous JWKS fetch did not fit) and ADR 0821 (the per-arm cross-site rule was not built)
(#3936).

**Decision.** A. The sum wrapper already awaits each member in declared order, so an `Oidc` arm
verifies exactly as the single-actor seam does (the same awaited JWKS verification, with `trustFrom`
replacing the declared roots) and a `SignedCookie` arm as its seam does. The two codes
`bynk.actor.oidc_not_in_sum` and `bynk.actor.signed_cookie_not_in_sum` are retired (tombstoned). B. The
cross-site check belongs to the cookie arm: the arm reads the session cookie first, and only with one
present, on a state-changing method, checks `Sec-Fetch-Site`/`Origin`. A failure there is that arm
failing: the next member is tried, and if none resolves the answer is `403`, not `401`. With no cookie
the arm refuses nothing, so a token client never meets a browser rule, and a request with no
credential at all is `401`. C. A request carrying a cookie and a token is resolved by declared order; the
reference recommends listing the token arm first, so an explicit token wins over an ambient cookie. A
cross-site request cannot succeed through the cookie by attaching a forged token, because a forged token
fails its arm and the cookie arm's check then refuses it. D. Still refused: two members of one scheme
(`bynk.actor.duplicate_sum_scheme`), a refinement in a sum (`bynk.actor.refinement_in_sum`), and a
refinement over `Oidc` (#918). E. A `system` case acts as either member (`by User("ada")`,
`by ApiClient("ci")`: the driver sends the member's own credential, and a case's own `Sec-Fetch-Site`
wins over the cookie arm's same-origin default); `by Nobody` is the no-credential request. `bynk openapi`
lists each member as an alternative `security` requirement (a cookie `apiKey`, a bearer `http` scheme),
documents the `401`, and the `403` on a state-changing route with a cookie arm. Hover on the `by` of a
sum lists the members in the order they are tried and the check each arm runs.

**Consequences.** **Security-relevant (authentication):** a request is now resolved by whichever arm
verifies first, so the order is part of a route's behaviour and a security decision for a request that
carries both credentials (list the token first). The cross-site protection of a cookie is unchanged on
a cookie-only route and on the cookie arm of a sum for a request carrying a cookie: a request carrying a cookie
and a token is judged by whichever arm is listed first, and when the cookie arm is first and the request
is cross-site, that arm refuses it and the token arm is then judged on its own token. Not covered by this change: a sum arm reading `.claims` of its `Oidc` or `Bearer` identity
(deferred to #4344, with the `Bearer` sum arm), and the failure principals `by Forged`, `by Expired`
and `by Stale` on an `Oidc` or cookie arm of a sum: they act on the first `Bearer` or `Signature` member,
as before, and `by Nobody` covers a sum with no credential. The retired codes are tombstoned for
`bynk explain`.
