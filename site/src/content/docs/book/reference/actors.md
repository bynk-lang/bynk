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
| `Bearer` | `secret = "<ENV>"` | a string-constructible type | `http` | JWT HS256 over `Authorization: Bearer …`; `sub` → identity; `401` |
| `Signature` | `secret`, `header`, optional `timestamp` + `tolerance` | none | `http` | HMAC-SHA256 over the raw body; `401` |
| `Oidc` | `issuer`, `audience`, `jwks` (all URLs/ids; **no secret**); optional `trustFrom` | a string-constructible type | `http` | RS256/ES256 JWT verified against the provider's JWKS over `Authorization: Bearer …`; `iss`/`aud`/`exp` checked; `sub` → identity, and the verified claims → [`claims`](#reading-claims); `401` |
| `Internal` | — | `()`, or `CallerId` for `Caller` | `call`/`cron`/`queue` | a call's per-edge signature proves the calling context; cron and queue are platform-dispatched |
| `Foreign` | `secret = "<ENV>"`, optional `worker = "<name>"` | none | `call` | the caller, a Worker that is not Bynk, signs the call as the actor under the secret ([wire format](/book/reference/wire-format/#foreign-callers)); `401` unsigned, `403` where the handler does not admit it |

- **`secret`** values name an environment variable (the source the `Secrets`
  capability reads), not the key itself.
- A `Bearer` or `Oidc` actor's **`identity`** must be a context-owned,
  string-constructible type, so the minted value is sealed to the context.
- A `Signature` actor takes **no** `identity` and a handler using it **must** take
  a `body`; it is HTTP-only.
- An **`Oidc`** actor names **no secret** — its trust root is the provider's
  public JWKS. It is HTTP-only and, this slice, single-actor only (not a sum
  member, not a refinement base); `alg: none` and symmetric `HS*` are rejected.
- A **`Foreign`** actor is a caller that is not Bynk, declared by the context it
  calls. Its name starts with a capital letter, so it never reads as a
  context's; it takes **no** `identity` (the actor is the identity); its
  `secret` must be a string, may not be a `BYNK_CALL_` edge key's name, and may
  not be any other actor's secret in the context. `bynk deploy` mints the
  secret, sets it on the context, and prints the `wrangler secret put` for the
  foreign Worker's operator, naming the Worker when `worker` is given.

## Reading claims

An `Oidc` actor's binder carries the verified token's claims beside its
identity, as a `TokenClaims`:

```bynk,fragment
on POST("/release") () -> Effect[HttpResult[String]] by w: Workload {
  let repository = w.claims.get("repository")     // Option[String]
  let protected = w.claims.has("environment")     // Bool
  Ok(repository.getOrElse("-"))
}
```

| Method | Returns | Reads |
|---|---|---|
| `get(name)` | `Option[String]` | the claim when it is a JSON string; `None` for any other type, or none at all |
| `has(name)` | `Bool` | whether the claim is present, of any type |

Every claim comes from a token whose signature, `iss`, `aud` and `exp` the seam
verified; a token that fails is `401` before the handler runs. The claims are
read where the token arrives and never kept: a `TokenClaims` can't be a field, or
cross a call, a message or an agent (`bynk.types.claims_at_boundary`). Read the
claim you need and keep that `String`. Only an `Oidc` binder has `.claims`;
`Bearer` claims are a later addition.

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
falls back to the declaration. It is how a test points the actor at a fake
issuer, and how a staging deployment trusts a staging one. Like a `secret`, the
variable is the operator's configuration. `trustFrom` must be a string
(`bynk.actor.oidc_trust_from_not_string`) naming a variable no actor uses as its
`secret` (`bynk.actor.oidc_trust_from_is_secret`).

## The `by` clause

```bynk,ignore
by <binder>: <Actor>            // capture the identity as <binder>.identity
by <Actor>                      // verify, capture nothing (optional binder)
by <binder>: <A> | <B> | …      // an ordered sum of peer actors, first-wins
```

- **HTTP requires a `by` clause** (`bynk.actor.missing_by_on_http`); a public
  route writes `by v: Visitor`. The internal protocols default: `call` →
  `Caller`, cron → `Scheduler`, queue → `Producer`.
- **Multi-actor sums** resolve first-wins; the body `match`es the resolved actor.
  Peers are distinguished by scheme (no duplicates), a `None` catch-all is last,
  refinements are not members, and total failure is `401`. On `call`, a sum
  admits a Bynk context (`Caller`) and declared foreign callers, each told apart
  by name: `by who: Caller | Legacy | Ledger`. Several `Foreign` peers may share
  a sum; each has its own secret.
- **Refinement actors** (`<Admin> = <User> where <pred>`) add an authorisation
  invariant over a `Bearer` base: scheme failure is `401`, predicate failure is
  `403`. The predicate is a closed set — `hasClaim("n")`, `claimEquals("n","v")`,
  composed with `&&`/`||`/`!`. An `Admin` is usable wherever its base is.
- **`Caller`** (on `on call`) yields the calling context's qualified name as
  `CallerId`. It admits Bynk contexts only: a foreign caller is admitted by its
  own actor, never by `Caller`, so a handler without `by` refuses one (`403`).

## Prelude actors

| Name | Scheme | Identity | For |
|---|---|---|---|
| `Visitor` | `None` | `()` | anonymous HTTP routes |
| `Caller` | `Internal` | `CallerId` | the calling context (`on call`) |
| `Scheduler` | `Internal` | `()` | cron |
| `Producer` | `Internal` | `()` | queue |

## Diagnostics

All `bynk.actor.*` codes are in the [diagnostic index](/book/reference/diagnostics/) — among
them `missing_by_on_http`, `scheme_not_admissible`, `signature_requires_body`,
`sum_requires_binder`, `duplicate_sum_scheme`, `unreachable_sum_arm`,
`refinement_base_unsupported`, `refinement_predicate_unsupported`,
`oidc_missing_issuer`, `oidc_missing_audience`, `oidc_missing_jwks`,
`oidc_identity_not_string_constructible`, `oidc_not_in_sum`,
`foreign_missing_secret`, `foreign_reserved_secret`,
`foreign_name_is_context_name`, `foreign_identity_unsupported`,
`foreign_secret_not_string`, and `foreign_duplicate_secret`.

## Who, not whose

An actor answers **who** is at the boundary — it authenticates a party and seals
its identity. It deliberately does **not** answer **whose** a given object is:
object-level authorisation (may *this* user read *this* record?) is domain logic
in the handler body, by design. A `where`-clause invariant narrows *who* (a claim
the party carries), never *whose*.

The normative rules are [Specification §5.7a](/book/spec/static-semantics/); the
emitted verification is [§7.3.4a](/book/spec/emission/).
