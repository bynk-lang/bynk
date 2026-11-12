---
level: minor
changelog: Docs for the `SignedCookie` actor scheme (#3386): a guide recipe, the reference, testing, `examples/sessions`, and an ADR; closes #3181
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
  under `--force`; `bynk dev` uses a fresh key per run.
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

**Consequences.** A service can authenticate a browser with no store and no hand-written cookie. A
stateless cookie cannot be revoked before `ttl`; ending the session is `expire()` (this browser) or
rotating the key (everyone). Store-backed sessions, a refresh flow, and encrypted cookies are deferred
and would be separate schemes or options. The scheme adds a runtime module, a deploy-time managed
secret, and a new principal-rule arm; nothing existing changes meaning.
