---
level: minor
changelog: `bynk openapi` documents a `SignedCookie` route's cookie scheme, hover, completion and signature help know `User.issue` and `User.expire`, and the scheme has a guide, reference, an example cross-reference and an ADR (#3386); closes #3181
probes: emit_diagnostics, diagnostic_coverage, explained_codes
---

## ADR: signed-cookie-actor
title: A SignedCookie actor scheme: a stateless, signed session cookie Bynk issues and verifies
summary: The scheme's shape, cookie format, key management, CSRF rule, and what is deferred

**Context.** The platform's sessions example keeps the session on the server and builds the cookie by
hand. Nothing in the language could authenticate a browser from a cookie, so a service either ran a
store or accepted a bearer token minted elsewhere (#3181 deferred the cookie-backed scheme). A cookie
that carries its own proof avoids the store, but a hand-rolled one is easy to get wrong (unsigned
value, no expiry, weak flags, no CSRF defence, a credential visible to the handler).

**Decision.** A `SignedCookie` actor scheme: `auth = SignedCookie(secret = "<ENV>", ttl = <duration>)`
with optional `cookie`, `previous` and `sameSite`, an identity minted from the cookie's subject, HTTP
only, standing alone on its route (never a sum member or a refinement base, and not on a public
`@cache` route).

- **Format and signing.** A compact JWS `header.payload.mac`, HMAC-SHA-256 through WebCrypto
  (`crypto.subtle.verify`, constant time). The header is `{alg: HS256, typ: bynk+session, kid}` and the
  payload `{sub, aud, iat, exp}` with `aud` the actor's name. The cookie is **signed, not encrypted**:
  the identity is not a secret, and encrypting would add a second key and a nonce for no property the
  scheme claims. The verifier also refuses a lifetime over the actor's current `ttl`, so shortening
  `ttl` shortens cookies already out, and a missing, repeated or oversized cookie.
- **Keys.** Unpadded base64url of at least 32 bytes; a bad key is a faulted request, never a quiet
  accept. `previous` is accepted for verification only, selected by `kid`, so a key rotates without
  signing everyone out. `secret` and `previous` must differ from each other and from other actors'
  secrets. `bynk deploy` mints a key when the variable is unset, keeps a held one, and replaces it only
  under `--force`; `bynk dev` uses a local key derived per binding. Contexts that share a binding share one minted
  key, and a deploy that would split them is refused.
- **`ttl` is required** (a session has no sensible default), and over 30 days warns. The default name
  is `__Host-session`; an unprefixed name warns. `Domain` and `Path` are not configurable.
- **Issuing.** `Actor.issue(identity)` is an effect needing `Clock`, and `Actor.expire()` is the
  matching deletion; both are available only inside the declaring context, since an actor is sealed by
  scope, so no other context can mint the identity. The handler never sees the credential: the router
  omits the cookie from its `cookies` record, `RequestCookies` view and `RequestHeaders` view.
- **CSRF.** The seam checks fetch metadata on `POST`, `PUT`, `PATCH` and `DELETE`: only
  `Sec-Fetch-Site: same-origin` passes on its own; `same-site` and `cross-site` need an `Origin` that
  matches **exactly** one of the service's credentialed `cors` origins; a wildcard or `null` never
  qualifies; `none` is refused on an unsafe method whatever its `Origin`; a request revealing neither
  header is refused; when `Sec-Fetch-Site` is absent the request's own origin passes. Failure is `403`.
- **Testing.** `by User("ada")` at every tier; at `system` the driver signs a real cookie and
  `by Nobody`, `by Forged` and `by Expired` probe the seam. The driver sends `same-origin` by default
  so a case's own headers probe the CSRF check, and an end-to-end matrix pins the rule.

**Deviations from the accepted proposal.** What shipped differs from #3386 in these ways, each made
during the build and each a tightening rather than a loosening:

- **`bynk.actor.signed_cookie_issue_outside_handler` is new.** The proposal listed
  `signed_cookie_issue_outside_context`; an actor is sealed by scope, so another context cannot name
  it and that code is unreachable and was not added. The new code refuses `Actor.issue` in a provider or
  an agent instead, because the signing key reaches a service handler's dependencies and not theirs:
  the call compiled but would have faulted on every request.
- **One signing key per binding across contexts.** Contexts that list the same `secret` variable (a
  login service that issues the cookie, an API that it guards) must hold the same value. `bynk deploy`
  mints it once and sets it on each, and refuses a run that would split them (a sibling already holds one,
  or `--context` reaches only some), since Cloudflare never returns a secret's value.
- **`bynk dev` uses a local key derived from the binding's name**, not a fresh key per run: distinct per
  actor, and the same in every `bynk dev --context` session so two side by side accept each other's
  cookies.
- **The handler's `RequestHeaders` view omits the session cookie too.** The proposal said the raw
  `headers.get("Cookie")` cannot be intercepted; the router can and does, as for `RequestCookies`.
- **`Sec-Fetch-Site: none` is refused on a state-changing request**, whatever its `Origin` (the
  proposal's sketch accepted `none`); a navigation is never a state-changing request.
- **A `SignedCookie` stands alone on its route.** A sum member was recommended (open question 10) but
  the per-arm CSRF rule is not built, so a sum is refused (`signed_cookie_not_in_sum`); the maintainer
  changed that answer to single-actor only.
- **`ttl` and the `Duration`/`Ident` config values** are parsed as typed values (`ttl = 12.hours`,
  `sameSite = Lax`) rather than as strings, and a sub-second `ttl` is an error.

**Consequences.** A service can authenticate a browser with no store and no hand-written cookie. A
stateless cookie cannot be revoked before `ttl`; ending the session is `expire()` (this browser) or
rotating the key (everyone). Store-backed sessions, a refresh flow, and encrypted cookies are deferred
and would be separate schemes or options. The scheme adds a runtime module, a deploy-time managed
secret, and a new principal-rule arm; nothing existing changes meaning.
