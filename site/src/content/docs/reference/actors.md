---
title: Actors
---
An **actor** is a nominal boundary contract — an authentication scheme plus an
optional sealed identity — that a handler consumes on its **`by` clause**. The
compiler emits the verification at the boundary, before the body runs,
fail-closed. Actors are declared inside a `context`. For task-oriented recipes,
see [Guides — Actors & access control](/book/guides/actors/).

## Declaration

```bynk,ignore
actor <Name> { auth = <Scheme>(<config>) , identity = <Type> }   // base actor
actor <Name> = <Base> where <predicate>                          // refinement
```

## Schemes

A closed, compiler-known set. `auth = <Scheme>` with keyed config where noted.

| Scheme | Config | Identity | Protocols | Verification |
|---|---|---|---|---|
| `None` | — | `()` | any | none (anonymous; the prelude `Visitor`) |
| `Bearer` | `secret = "<ENV>"`; optional `issuer`, `audience`, `trustFrom` | a string-constructible type | `http`, `websocket` | JWT HS256 over `Authorization: Bearer …`; `sub` → identity, and the verified claims → [`claims`](#reading-claims); `401` |
| `Signature` | `secret`, `header`, optional `timestamp` + `tolerance` | none | `http` | HMAC-SHA256 over the raw body; `401` |
| `SignedCookie` | `secret = "<ENV>"`, `ttl`, optional `cookie`, `previous`, `sameSite`, `sessions` | a string-constructible type | `http`, `websocket` | a signed, expiring session cookie that Bynk issues and verifies; `sub` → identity; `401`. see [Signed cookies](#signed-cookies) |
| `Oidc` | `issuer`, `audience`, `jwks` (all URLs/ids; **no secret**); optional `trustFrom` | a string-constructible type | `http`, `websocket` | RS256/ES256 JWT verified against the provider's JWKS over `Authorization: Bearer …`; `iss`/`aud`/`exp` checked; `sub` → identity, and the verified claims → [`claims`](#reading-claims); `401` |
| `Internal` | — | `()`, or `CallerId` for `Caller` | `call`/`cron`/`queue` | a call's per-edge signature proves the calling context; cron and queue are platform-dispatched |
| `Foreign` | `secret = "<ENV>"`, optional `worker = "<name>"` | none | `call` | the caller, a Worker that is not Bynk, signs the call as the actor under the secret ([wire format](/reference/wire-format/#foreign-callers)); `401` unsigned, `403` where the handler does not admit it |

- **`secret`** values name an environment variable (the source the `Secrets`
  capability reads), not the key itself.
- A `Bearer` or `Oidc` actor's **`identity`** must be a context-owned,
  string-constructible type, so the minted value is sealed to the context.
- A route guarded by a `Bearer`, `Oidc`, `SignedCookie` or `Signature` actor sends
  `Cache-Control: private` on every response unless it declares its own `@cache`
  ([Authenticated responses](/reference/http/#authenticated-responses)).
- The seam **consumes** a `Bearer`, `Oidc` or `Signature` credential, as it does a `SignedCookie`'s: the
  handler's `headers: RequestHeaders` omits the `Authorization` header (so `bearer()` is `None`), or the
  signature header, and the handler sees only the identity ([Request headers](/reference/http/#request-headers)).
- On a **WebSocket**, a `Bearer` or `Oidc` token's `exp`, or a session cookie's, is checked once at the upgrade and again at every frame: a
  frame that arrives after it closes the connection with `1008 authentication expired` before
  `on message` runs ([WebSocket](/reference/websocket/#a-credential-that-expires-on-a-live-connection)).
- A `Signature` actor takes **no** `identity` and a handler using it **must** take
  a `body`; it is HTTP-only.
- An **`Oidc`** actor names **no secret** — its trust root is the provider's
  public JWKS. It is HTTP-only and may be a member of a sum ([a cookie or a token on one route](#a-cookie-or-a-token-on-one-route)), but
  not a refinement base; `alg: none` and symmetric `HS*` are rejected, as is a
  token whose `sub` is empty or missing. A `system` suite can show the seam does so
  (`by WrongAlgorithm`, `by NoSubject`; see
  [the failure principals](/reference/testing/#failure-principals)).
- A **`Foreign`** actor is a caller that is not Bynk, declared by the context it
  calls. Its name starts with a capital letter, so it never reads as a
  context's; it takes **no** `identity` (the actor is the identity); its
  `secret` must be a string, may not be a `BYNK_CALL_` edge key's name, and may
  not be any other actor's secret in the context. `bynk deploy` mints the
  secret, sets it on the context, and prints the `wrangler secret put` for the
  foreign Worker's operator, naming the Worker when `worker` is given.

## Signed cookies

A `SignedCookie` actor authenticates a browser from a session cookie that Bynk itself issues
and verifies, so there is no token to mint elsewhere:

```bynk,ignore
actor User {
  auth = SignedCookie(
    cookie = "__Host-session",       // optional; this is the default
    secret = "SESSION_KEY",          // env var holding the signing key
    previous = "SESSION_KEY_PREV",   // optional; accepted for verification only
    ttl = 12.hours                   // required
  ),
  identity = UserId
}
```

| Key | Meaning |
|---|---|
| `secret` | the environment variable holding the signing key (never the key). Required |
| `ttl` | how long an issued cookie is valid, a positive whole number of seconds. Required; over 30 days warns |
| `cookie` | the cookie's name; defaults to `__Host-session`; a name without the `__Host-` prefix warns |
| `previous` | the key being rotated out, accepted for verification only |
| `sameSite` | `Lax` (the default), `Strict` or `Unrestricted` |
| `sessions` | `true` keeps each session in a generated agent the seam checks on every request, so a session can be ended on the server (see [Revocable sessions](#revocable-sessions)); `false`, the default, keeps the cookie stateless |

The cookie's `Domain` and `Path` are not configurable: it is always host-only with `Path=/`.
The identity is minted from the cookie's `sub` and must be a string-constructible type the
context owns, as for `Bearer`. A `SignedCookie` secret may not be any other actor's secret.

**What the seam does.** A route guarded by a `SignedCookie` actor runs, before the body:

1. **A CSRF check**, on `POST`, `PUT`, `PATCH` and `DELETE` (a `GET` is safe and skips it). Only a request
   whose `Sec-Fetch-Site` is `same-origin` passes on its own. Any other (`same-site`, `cross-site`, `none`, a
   user-initiated navigation that a state-changing request is not, or an unknown value) must carry an
   `Origin` that is **exactly** one of the service's [`cors`](/reference/http/#cors) origins (scheme,
   host and port; the service must declare `credentials: true`, a sibling subdomain counts only if it is
   listed, and a wildcard or `null` never qualifies). When the browser sent no `Sec-Fetch-Site` at all, this
   request's own origin passes too. A request that reveals neither header is refused. Failure is `403`.
2. **Reading the cookie.** A missing cookie, or one sent more than once (a sign of cookie tossing
   from another path or subdomain), is `401`.
3. **Verifying it.** HMAC-SHA-256 under `secret`, or `previous` when the cookie's `kid` names it;
   the cookie must be for this actor, unexpired, issued no later than now, and valid for no longer
   than the actor's current `ttl`. Any failure is `401`. A signing key that is missing, not
   base64url, or shorter than 32 bytes faults the request (`500`, and the log names the variable),
   never a quiet accept.
4. **Minting the identity** from the verified `sub`; a `sub` that violates the identity type's
   refinement is `401`.
5. **Checking the session**, only with `sessions = true`: the cookie must carry a session id, and the
   identity's session agent must hold it live. A cookie without one, or one the agent has dropped, is
   `401`. The check runs after the signature, so a forged cookie never costs an agent call.

## Issuing and deleting the cookie

Inside the context that declares it, a `SignedCookie` actor has two members (three with `sessions = true`:
`revokeAll`, below), so no other context can mint its identity:

```bynk,fragment
on POST("/login") (body: LoginRequest) -> Effect[HttpResult[String]] by v: Visitor given Clock {
  let Ok(id) = UserId.of(body.user) else { BadRequest("bad user") }
  let c <- User.issue(id)               // Effect[Cookie]
  Created("welcome").withCookie(c)
}

on POST("/logout") () -> Effect[HttpResult[()]] by u: User {
  NoContent.withCookie(User.expire())   // Cookie
}
```

`User.issue(identity)` takes the actor's identity type and answers an `Effect[Cookie]`: the signed
cookie, valid for the actor's `ttl`. It reads the time from the [`Clock`](/reference/bynk-capabilities/),
so the handler lists `Clock` in `given` (`bynk.actor.signed_cookie_issue_needs_clock`), and a test can stub
it. It can only be called in a service handler, where the signing key arrives, not in a provider or an agent
(`bynk.actor.signed_cookie_issue_outside_handler`). The `Cookie` it returns is built by the scheme, not by you: the configured name,
`HttpOnly`, `Secure`, `Path=/`, no `Domain`, the actor's `SameSite` and a `Max-Age` equal to `ttl`.
There are no attributes to get wrong, so you attach it with `withCookie` and nothing else.
`User.expire()` is the matching deletion (same name and path, empty, `Max-Age=0`), so a logout cannot
miss the cookie it set.

A stateless cookie (the default) **cannot be revoked** before it expires: a copy a user already took keeps
working until then. `expire` removes it from this browser; it does not invalidate the cookie. Keep the `ttl`
short, or set `sessions = true` (below) so a session can be ended on the server.

### Revocable sessions

`SignedCookie(…, sessions = true)` makes each session a server-side record. The cookie carries a session id
(`sid`), and the compiler generates an agent, `<Actor>Sessions`, keyed by the identity, that holds that
identity's live sessions with their expiry. It appears in the schema lock and is deployed and migrated like
any agent you write; its shape is the compiler's to evolve.

- `User.issue(identity)` adds a session to the identity's agent before it signs the cookie, so a cookie
  never exists for a session the agent does not know. Expired sessions are dropped as new ones are added.
- `User.expire()` ends the *current* session in the agent, then answers the deleting cookie. With
  `sessions = true` it is an `Effect[Cookie]` (bind it with `<-`) and, like `issue`, can only be called in
  a service handler. A handler the actor does not guard has no session to end and only gets the cookie.
- The seam asks the agent on every request, after the signature check. A session that was ended, has
  expired in the agent, or never existed is `401`, so a copy of an `expire`d cookie replayed from another
  browser is refused.

Because every authenticated request makes one agent call, `sessions` is opt-in and the stateless cookie stays
the default. A hot identity is one user's own sessions, so contention is unlikely. A cookie signed before
`sessions` was switched on carries no session id and is refused (`401`): switching it on logs everyone out.

`User.revokeAll(identity)` ends **every** session of one identity: an `Effect[()]` that takes the actor's
identity type and, like `issue`, can only be called in a service handler. Use it to log out everywhere and
after a password change or an account compromise; another identity's sessions are untouched. It exists only
with `sessions = true` (a stateless actor has no sessions to revoke: `bynk.types.method_not_found`).

```bynk,fragment
on POST("/password") (body: NewPassword) -> Effect[HttpResult[()]] by u: User given Accounts {
  let _ <- Accounts.setPassword(u.identity, body)
  let _ <- User.revokeAll(u.identity)   // every session for this user, this one included
  NoContent
}
```

In a handler guarded by an [actor sum](/book/guides/actors/multiple-callers/) that has a cookie member, the
cookie arm checks the session the same way and `expire()` ends the verified session, so a sum route logs out
like a single-actor route.

Revocation takes effect on the next request, not on the response that ends it: a request already past the
seam finishes. An agent that cannot answer faults the request (`500`) rather than admitting it.

The handler never sees the credential: the router leaves the session cookie out of the handler's
`cookies` record and raw `RequestCookies` view, and a record that names it is refused
(`bynk.actor.signed_cookie_read_directly`). `bynk deploy` mints the `secret` variable when it is unset (see [Signed cookies](/book/guides/projects-build-and-deployment/deploy-to-cloudflare/#session-keys)).
A `system` suite signs in as `by User("ada")`; with `sessions = true` that is one session per principal and
case, like one browser, so a later call in the case replays the same cookie and sees a session that `expire()`
or `revokeAll` ended as `401`. It probes the seam with `by Nobody`, `by Forged` and
`by Expired`, and, with `sessions = true`, `by Revoked` (see
[the failure principals](/reference/testing/#failure-principals)); a case's own
`Sec-Fetch-Site` and `Origin` headers probe the CSRF check.

`bynk openapi` documents a route a `SignedCookie` actor guards with an `apiKey` security scheme
`in: cookie`, named after the cookie, and lists the seam's refusals as responses: `401` always and, on a
state-changing method, `403` for a cross-site request.

### What a signed cookie does and does not protect against

It **does**: reject a cookie that was forged, altered, issued for another actor, signed with a key the
seam does not hold, expired, or sent twice; refuse a cross-site state-changing request that the service
does not list; keep the credential out of the handler (its `cookies` and `headers` views omit it); and
keep the cookie from page scripts (`HttpOnly`), from plain HTTP (`Secure`), and from sibling subdomains
(`__Host-` and no `Domain`).

It does **not**: hide the cookie's contents. The value is **signed, not encrypted**: anyone holding the
cookie can read its `sub`, so never put a secret in an identity. It cannot be revoked before `ttl`
(above). It does not stop a script already running on your own origin, and it is not a defence against a
stolen device. Rotating `secret` (moving the old value to `previous`, then dropping it) is how every
cookie is ended at once.

A `SignedCookie` actor may be a member of a sum, so one route admits a browser's session cookie or an
API client's token (below). It cannot be the base of a refinement actor, and cannot guard a
`@cache(scope: public)` route (`bynk.actor.signed_cookie_public_cache`).

### A cookie or a token on one route

```bynk,fragment
actor User      { auth = SignedCookie(secret = "SESSION_KEY", ttl = 12.hours), identity = UserId }
actor ApiClient { auth = Oidc(issuer = "https://id.example", audience = "api", jwks = "https://id.example/jwks"), identity = UserId }

service api from http {
  on POST("/orders") (body: NewOrder) -> Effect[HttpResult[Order]] by who: ApiClient | User {
    match who {
      ApiClient(c) => Ok(c)
      User(u)      => Ok(u)
    }
  }
}
```

The members are checked **in the order they are written**, and the first that verifies is the actor the
body matches on. Each arm runs its own seam, so the cookie arm's cross-site check belongs to the cookie arm:

- A request with **no session cookie** never meets it. A token client that sends only `Authorization`
  is checked by the token arm, whichever is listed first.
- A **state-changing** request with a session cookie must pass the cross-site check
  (`Sec-Fetch-Site: same-origin`, or an `Origin` among the service's credentialed `cors` origins) for
  the cookie arm to resolve. A cross-site request carrying **only** a cookie is refused `403`, and the
  same request carrying a valid token is admitted by the token arm.
- A request that no arm resolves is `401`, as for any sum, unless the cookie arm refused it as cross-site,
  which is `403`.
- A request carrying **both** a cookie and a token is resolved by the order. List the token arm first
  when clients may hold both, so an explicit token wins over an ambient cookie, and a cross-site
  page cannot succeed through the cookie by attaching a forged token.

An arm's identity is its `identity`, as in any sum, and a `Bearer` arm reads its verified claims through a
second binder ([In an actor sum](#in-an-actor-sum)). Two
cookie members, or two members of one scheme, are still refused (`bynk.actor.duplicate_sum_scheme`), as is
a refinement in a sum (`bynk.actor.refinement_in_sum`).

## Reading claims

The binder of an `Oidc` actor, of a `Bearer` actor, and of a refinement of a
`Bearer` actor carries the verified token's claims beside its identity, as a
`TokenClaims`:

```bynk,fragment
on POST("/release") () -> Effect[HttpResult[String]] by w: Workload {
  let repository = w.claims.get("repository")     // Option[String]
  let protected = w.claims.has("environment")     // Bool
  Ok(repository.getOrElse("-"))
}
```

A `Bearer` binder reads them the same way, so a handler behind an ordinary HS256 JWT can read a
tenant, a scope or a plan:

```bynk,fragment
on GET("/reports") () -> Effect[HttpResult[String]] by u: User {
  let tenant = u.claims.get("tenant").getOrElse("default")
  Ok(tenant)
}
```

A refinement's binder (`by a: Admin`, where `actor Admin = User where hasClaim("admin")`) carries its
base's claims, the same token's. The claims are the token's whether or not a refinement's predicate
reads them. A route with no binder (`by User`) verifies the token and reads nothing.

| Method | Returns | Reads |
|---|---|---|
| `get(name)` | `Option[String]` | the claim when it is a JSON string; `None` for any other type, or none at all |
| `has(name)` | `Bool` | whether the claim is present, of any type |

Every claim comes from a token whose signature, `iss`, `aud` and `exp` the seam
verified; a token that fails is `401` before the handler runs. The claims are
read where the token arrives and never kept: a `TokenClaims` can't be a field, or
cross a call, a message or an agent (`bynk.types.claims_at_boundary`). Read the
claim you need and keep that `String`. Only the binder of an `Oidc` or a `Bearer`
actor has `.claims` (not another scheme's, and not a sum's: the binder of `by who: A | B` is the
resolved arm, which reads claims as below).

### In an actor sum

The binder of an actor sum is the resolved arm, so it has no `.claims`. A `Bearer` or an `Oidc` arm of an
`http` handler's sum binds the verified claims with a **second pattern binder**, beside its identity:

```bynk,fragment
on GET("/who") () -> Effect[HttpResult[String]] by who: User | Visitor {
	match who {
		User(u, c) => Ok("\(u) \(c.get("tenant").getOrElse("default"))")   // c: TokenClaims
		Visitor => Ok("anonymous")
	}
}
```

The one-name form, `User(u)`, is unchanged, and an arm that binds no claims reads none. The claims are the
verified token's, as for a lone `Bearer` or `Oidc` binder: an arm resolves only after its token's signature
and checks pass, and a `Visitor` or any other arm has none. Only a `Bearer` or `Oidc` arm of an `http` sum has
the second binder: a second binder on any other arm (`Signature`, `SignedCookie` or an open one), on a sum of
another protocol, or a third on a `Bearer` or `Oidc` arm, is `bynk.types.pattern_arity`. A test supplies the
claims as the principal's second argument, as for a lone actor: `by User("ada", TokenClaims.of([…]))`.

A claim says what the token asserts, not whose data the handler may touch: an object-level check
(*is this report this tenant's?*) stays in the handler. See [Who, not whose](#who-not-whose).

## Issuer and audience for a `Bearer` token

A `Bearer` token is an HS256 signature over its claims, so any token signed with the actor's secret is
accepted, whatever it was minted for. Where several kinds of token share one secret (a web app's token and an
internal one, say), bind each to its purpose with the `iss` and `aud` claims:

```bynk,ignore
actor Learner  { auth = Bearer(secret = "AUTH_SECRET", issuer = "agilearn-web", audience = "agilearn-api"), identity = LearnerId }
actor Internal { auth = Bearer(secret = "AUTH_SECRET", audience = "agilearn-internal"), identity = LearnerId }
```

- **`issuer`**: the token's `iss` must be present and equal it.
- **`audience`**: the token's `aud` must be present and equal it, or, when `aud` is an array of strings, contain it.
  An `aud` of any other shape is refused.
- Either missing from a token, or different, is `401`, checked after the signature and the expiry, so a forged token
  says nothing about them. An actor that declares **neither** checks neither, as before: a token carrying an `iss`
  or `aud` it does not name is still accepted.
- **`trustFrom = "<ENV>"`** names an environment variable whose JSON `{ "issuer"?, "audience"? }`, when set,
  replaces the declared values, exactly as for [`Oidc`](#trust-roots-from-the-environment) (unset leaves the
  declaration, set but malformed refuses every request). It must name a variable other than the `secret`.
- **Two `Bearer` actors on one secret** must each declare an `audience`, and different ones
  (`bynk.actor.bearer_secret_shared`); otherwise a token minted for one is accepted by the other. Declare a
  distinct `audience` on each and mint each actor's tokens with its own, which `bynk token <Actor>` does. A
  refinement of an actor shares its base's secret by design and is exempt.
- The config keys are a closed set: `secret`, `issuer`, `audience` and `trustFrom`. Any other is
  `bynk.actor.bearer_unknown_key`, because an ignored key is a check you believed you had written, and an empty or
  non-string value is `bynk.actor.bearer_bad_arg`.
- `bynk token <Actor>` mints with the declared `iss` and `aud`, and the `system` tier signs a valid principal's token
  with them. `by WrongIssuer` and `by WrongAudience` drive a route whose actor declares the claim
  ([failure principals](/reference/testing/#failure-principals)).

## Trust roots from the environment

`trustFrom = "<ENV>"` names an environment variable whose JSON, when it is set,
replaces the declared trust roots at run time:

```bynk,ignore
actor Workload {
  auth = Oidc(
    issuer = "https://token.actions.githubusercontent.com",
    audience = "bynk-registry",
    jwks = "https://token.actions.githubusercontent.com/.well-known/jwks",
    trustFrom = "WORKLOAD_OIDC_TRUST"
  ),
  identity = WorkloadId
}
```

```json
{ "issuer": "http://127.0.0.1:8123", "jwks": "http://127.0.0.1:8123/jwks", "audience": "bynk-registry" }
```

`issuer` and `jwks` are required, `audience` optional. Unset, the declaration
applies, so production behaviour is exactly what the actor says; set but
malformed, every request is refused (`401`) and the fault log says why — it never
falls back to the declaration. It is how a staging deployment trusts a staging issuer, and how a `system`
suite's `env` moves the trust roots the test driver mints for (the driver is the
issuer; see [testing an `Oidc` route](/reference/testing/#oidc-issuer)). Like a `secret`, the
variable is the operator's configuration. `trustFrom` must be a string
(`bynk.actor.oidc_trust_from_not_string`) naming a variable no actor uses as its
`secret` (`bynk.actor.oidc_trust_from_is_secret`).

## The `401` challenge

Every `401` an HTTP route (or a WebSocket upgrade) answers carries a `WWW-Authenticate`
header, as RFC 9110 §15.5.2 requires. The challenge follows the route's `by` actor and
names the context as its `realm`:

| Actor on the route | Challenge |
|---|---|
| `Bearer`, `Oidc` | `Bearer realm="<context>"`, plus `error="invalid_token"` when the request presented a bearer token (RFC 6750 §3); a request with no token gets the bare challenge, and there is never an `error_description` |
| `Signature` | `Bynk-Signature realm="<context>", header="<the actor's header>"` |
| `SignedCookie` | `Bynk-Cookie realm="<context>"` |
| a sum of actors | one challenge per member, in the sum's order |
| `Visitor`, or any other route | `Bynk realm="<context>"` |

`Bynk-Signature` and `Bynk-Cookie` are not registered schemes: HMAC signatures and cookies
have none, so these satisfy the RFC without implying a protocol a generic client might try
to speak. A handler's own `Unauthorized` carries the route's challenge too, and `on rejected`
keeps it when it renders a `401` body. A service with CORS adds `WWW-Authenticate` to
`Access-Control-Expose-Headers` so a browser client can read it.

## The `by` clause

```bynk,ignore
by <binder>: <Actor>            // capture the identity as <binder>.identity
by <Actor>                      // verify, capture nothing (optional binder)
by <binder>: <A> | <B> | …      // an ordered sum of peer actors, first-wins
```

- **HTTP, WebSocket and `call` require a `by` clause**, because the caller is a
  party and more than one actor is sensible. A public HTTP route writes
  `by v: Visitor` (`bynk.actor.missing_by_on_http`). An `on call` handler writes
  at least `by Caller`, the explicit "any consuming context"
  (`bynk.actor.missing_by_on_call`); `bynk check --fix=bynk.actor.missing_by_on_call`
  inserts it, and a service-header `by` covers every handler without its own.
  An `on message` handler writes at least `by Producer`, the explicit "anyone on the
  account" (`bynk.actor.missing_by_on_queue`); `bynk check --fix=bynk.actor.missing_by_on_queue`
  inserts it. A `Foreign` actor on an `on message` handler admits only messages signed under its
  secret, in [the signed-message format](/reference/wire-format/#signed-queue-messages):
  an unsigned or wrongly signed message is acknowledged, reported and never handled, and a
  replayed one likewise. A refinement of `Producer` (`actor Shop = Producer where
  IsContext(shop)`) admits only messages a Bynk producer in one of those contexts signed, and a
  `Queue` instance in such a context signs what it sends with nothing written. Hover on a
  `Producer` refinement, or on its name in a `by` clause, lists the contexts it admits and says
  it requires signatures. A `Foreign` actor or a
  refinement beside an unsigned one, or alone with a binder, is `bynk.actor.queue_actor_not_enforced`.
  Foreign actors and refinements may share a sum, `by who: Legacy | Shop`; a refinement's member carries
  the sending context's name. An email `on message` handler writes at least `by Sender`, the explicit
  "any mail" (`bynk.actor.missing_by_on_email`); `bynk check --fix=bynk.actor.missing_by_on_email`
  inserts it. The platform-only protocol defaults: cron → `Scheduler`. An agent's
  `on call` operations are reached only from inside
  their own context, so they name no caller.
- **Multi-actor sums** resolve first-wins; the body `match`es the resolved actor.
  Peers are distinguished by scheme (no duplicates), a `None` catch-all is last,
  refinements are not members, and total failure is `401`. `SignedCookie` and `Oidc` actors may be
  members ([a cookie or a token on one route](#a-cookie-or-a-token-on-one-route)). On `call`, a
  sum admits a Bynk context (`Caller`, or one refinement over it) and declared
  foreign callers, each told apart by name: `by who: Caller | Legacy | Ledger`.
  A sum has at most one `Internal` member (`bynk.actor.duplicate_sum_scheme`).
  Several `Foreign` peers may share a sum; each has its own secret.
- **Refinement actors** (`<Admin> = <User> where <pred>`) add an authorisation
  invariant over a `Bearer` base: scheme failure is `401`, predicate failure is
  `403`. The predicate is a closed set — `hasClaim("n")`, `claimEquals("n","v")`,
  composed with `&&`/`||`/`!`. An `Admin` is usable wherever its base is.
  `Caller` is a refinement base too, with its own predicate (next bullet).
- **Refinements over `Caller`** (`actor Admin = Caller where IsContext(shop.admin)`)
  name the contexts that may call a service. `IsContext(<context>, …)` takes one
  or more qualified context names, written as names rather than strings; each
  must be a context of the project (`bynk.actor.unknown_context`). The
  predicate is admissible only over `Caller` (`hasClaim` and `claimEquals` stay
  `Bearer`'s, and `IsContext` over a `Bearer` actor is
  `bynk.actor.refinement_predicate_unsupported`), and composes with `&&`, `||`
  and `!`, so `!IsContext(shop.legacy)` is a deny-list. The refinement inherits
  `Caller`'s scheme, its identity (`CallerId`) and its admissibility (`on call`
  only). A call from a context it does not admit is
  `bynk.actor.caller_not_admitted` at the call, in a case's `by Admin("…")` too;
  the callee also answers such a call `403` (wire format check 6). See
  [Restrict who may call](/book/guides/actors/cross-context-callers/#restrict-who-may-call).
- **Refinements over `Sender`** (`actor Authenticated = Sender where Dmarc(DmarcPass)`)
  admit only mail whose DMARC verdict is one of those listed, and the Worker
  refuses other mail with the reason `sender not authenticated` before the handler
  runs (a refusal, not a fault, so it is logged by outcome only). `Dmarc(<verdict>, …)`
  takes `DmarcPass`, `DmarcFail` or `DmarcNone`, composes with `&&`, `||` and `!`
  (`!Dmarc(DmarcFail)` admits a pass or no verdict), and is admissible only over
  `Sender`; over another base it is `bynk.actor.refinement_predicate_unsupported`.
  The refinement inherits `Sender`'s scheme and `SenderIdentity`, so `by s: Authenticated`
  binds the same record. `Sender` and its refinements share the `Internal` scheme, so at
  most one of them appears in a sum. Hover on the refinement, or on its name in a `by`
  clause, lists the verdicts it admits and says the Worker refuses the rest before the
  handler runs; a unit that declares its own actor named `Sender` shadows the prelude one,
  and then hover says nothing of the kind.
- **`Caller`** (on `on call`) yields the calling context's qualified name as
  `CallerId`. It admits Bynk contexts only: a foreign caller is admitted by its
  own actor, never by `Caller`, so a handler that names only `Caller` refuses
  one (`403`).

## Prelude actors

| Name | Scheme | Identity | For |
|---|---|---|---|
| `Visitor` | `None` | `()` | anonymous HTTP routes |
| `Caller` | `Internal` | `CallerId` | the calling context (`on call`) |
| `Scheduler` | `Internal` | `()` | cron |
| `Producer` | `Internal` | `()` | queue |
| `Sender` | `Internal` | `SenderIdentity` (minted by the Worker; `()` in a suite today) | [inbound email](/reference/inbound-email/#the-sender) |

## Diagnostics

All `bynk.actor.*` codes are in the [diagnostic index](/reference/diagnostics/) — among
them `missing_by_on_http`, `scheme_not_admissible`, `signature_requires_body`,
`sum_requires_binder`, `duplicate_sum_scheme`, `unreachable_sum_arm`,
`caller_not_admitted`, `unknown_context`,
`refinement_base_unsupported`, `refinement_predicate_unsupported`,
`oidc_missing_issuer`, `oidc_missing_audience`, `oidc_missing_jwks`,
`oidc_identity_not_string_constructible`,
`foreign_missing_secret`, `foreign_reserved_secret`,
`foreign_name_is_context_name`, `foreign_identity_unsupported`,
`foreign_secret_not_string`, `foreign_duplicate_secret`, and the `signed_cookie_*` family.

## Who, not whose

An actor answers **who** is at the boundary — it authenticates a party and seals
its identity. It deliberately does **not** answer **whose** a given object is:
object-level authorisation (may *this* user read *this* record?) is domain logic
in the handler body, by design. A `where`-clause invariant narrows *who* (a claim
the party carries), never *whose*.

The normative rules are [Specification §5.7a](/reference/spec/static-semantics/); the
emitted verification is [§7.3.4a](/reference/spec/emission/).
