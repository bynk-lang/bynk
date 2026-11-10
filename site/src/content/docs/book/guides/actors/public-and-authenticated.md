---
title: Serve public and authenticated routes
---
**Goal:** expose a public endpoint, and a second endpoint that requires a
verified user — reading the user's identity in the body.

## A public route: `Visitor`

HTTP has no safe default actor, so even an anonymous route declares one. The
prelude actor `Visitor` (scheme `None`) accepts everyone and yields no identity:

```bynk
context api

service api from http {
	on GET("/health") () -> Effect[HttpResult[String]] by Visitor {
		Ok("ok")
	}
}
```

The binder is optional — `by Visitor` (no name) verifies the contract and
captures nothing, which is all an anonymous route needs.

## An authenticated route: `Bearer`

A `Bearer` actor verifies a JWT from the `Authorization: Bearer …` header. It
names the env var holding the signing secret, and the identity type to mint from
the token's `sub` claim:

```bynk
context api

type UserId = String where NonEmpty

type Profile = {
	id: UserId,
}

actor User { auth = Bearer(secret = "AUTH_JWT_SECRET"), identity = UserId }

service api from http {
	on GET("/me") () -> Effect[HttpResult[Profile]] by u: User {
		Ok(Profile { id: u.identity })
	}
}
```

At the boundary, before the body runs, the compiler emits HS256 verification
(constant-time, with `exp`/`nbf` checks), mints `u.identity : UserId` from the
`sub` claim, and **fails closed with `401`** on any problem — a missing or
malformed token, a bad signature, an expired token, or a `sub` that does not
satisfy `UserId`'s refinement. Your body sees only a verified user.

- The **secret** is the name of an environment variable (the same source the
  `Secrets` capability reads), not the key itself.
- The **identity type** must be a context-owned, string-constructible type (here
  `UserId`), so the minted value is sealed to this context.
- `u.identity` is read-only. You cannot construct a `User`, pass it around, or
  reach any field other than `.identity`.

## Test a Bearer route's refusals

A `Bearer` actor's main claims are that a token signed with another secret is
refused and that an expired token is refused. A `system` suite checks both
against the real seam, with no script to mint tokens:

```bynk,fragment
suite api.activity as system {
	env {
		AUTH_JWT_SECRET = "test-secret"
	}

	case "tokens the seam must refuse are refused" {
		let none <- api.GET("/me") by Nobody  // no token
		let forged <- api.GET("/me") by Forged  // signed with a wrong secret
		let expired <- api.GET("/me") by Expired  // correctly signed, `exp` an hour ago
		expect none is Rejected(Unauthorized)
		expect forged is Rejected(Unauthorized)
		expect expired is Rejected(Unauthorized)
	}
}
```

`by Forged` and `by Expired` send an otherwise valid token (a good `sub`), so
only the signature or the `exp` is wrong. If the seam ever admitted one, the
call would read as `Handled(_)` and the case would fail. They are `system`-tier
only: below it the seam is skipped. See
[the failure principals](/book/reference/testing/#failure-principals) for the
full table.

## Verify a token without capturing the identity

Drop the binder when you only need to *gate* a route, not read who it was:

```bynk
context api

type UserId = String where NonEmpty

actor User { auth = Bearer(secret = "AUTH_JWT_SECRET"), identity = UserId }

service api from http {
	on POST("/ping") () -> Effect[HttpResult[String]] by User {
		Ok("pong")
	}
}
```

The token is still verified fail-closed; no identity is minted.

## Call a Bearer route by hand

To try a `Bearer` route with `curl`, or smoke-test a deployment, mint a token for
the actor with [`bynk token`](/docs/bynk-cli/#bynk-token). It signs with the
secret the actor names, read from the environment, so you never write a signing
script that could drift from the actor:

```bash
# Serve the project, giving the Worker the secret the actor declares.
bynk dev -- --var AUTH_JWT_SECRET:dev-only-secret

# In another terminal: the same secret, in the environment (never an argument).
export AUTH_JWT_SECRET=dev-only-secret
curl -H "Authorization: Bearer $(bynk token User --sub alice)" \
  http://localhost:8787/me
```

`--sub` must be a value the actor's identity type admits (`UserId` refuses `""`),
so the token is one the route accepts. To check that the route *refuses* what it
should, mint the bad ones:

```bash
# 401: expired an hour ago.
curl -i -H "Authorization: Bearer $(bynk token User --sub alice --expired)" \
  http://localhost:8787/me
# 401: not valid for another ten minutes.
curl -i -H "Authorization: Bearer $(bynk token User --sub alice --not-before 10m)" \
  http://localhost:8787/me
```

The default lifetime is one hour; `--lifetime 30m` sets another (at most 365
days). Only `Bearer` actors are supported.

## Set a service-level default

"Public unless stated otherwise" — or "authed unless stated otherwise" — is
usually a fact about the *whole service*, not each route. Write the default once
on the service header (after the protocol, `by` before `given`), and every handler
inherits it. A handler that names its own `by` overrides the default:

```bynk
context api

type UserId = String where NonEmpty

actor User { auth = Bearer(secret = "AUTH_JWT_SECRET"), identity = UserId }

service api from http by Visitor {
	// Inherits the default: public.
	on GET("/health") () -> Effect[HttpResult[String]] {
		Ok("ok")
	}

	// Overrides the default: this route requires a verified user.
	on GET("/me") () -> Effect[HttpResult[UserId]] by u: User {
		Ok(u.identity)
	}
}
```

The default fills only an *absent* clause — it never merges with a handler's own
`by`. A service-level `given` default works the same way for capabilities.

**Next:** [Add an authorisation invariant](/book/guides/actors/authorisation/) to require a claim
(an admin-only route), or [serve several kinds of caller](/book/guides/actors/multiple-callers/)
from one route.
