---
title: Authenticate a browser with a signed cookie
---
**Goal:** sign a user in from a browser with a session cookie, with no token to mint elsewhere and no
session store to run.

## The `SignedCookie` scheme

A `SignedCookie` actor authenticates a request from a cookie that Bynk itself issues. It names the
environment variable holding the signing key, how long a cookie lives, and the identity type minted from
the cookie's subject:

```bynk
context shop.api

type UserId = String where NonEmpty && MaxLength(64)

type LoginRequest = {
	user: String,
}

actor User { auth = SignedCookie(secret = "SESSION_KEY", ttl = 12.hours), identity = UserId }

service api from http {
	cors {
		origins: ["https://app.example.com"],
		credentials: true,
	}

	// The cookie is verified before the body runs; u.identity is the signed-in user.
	on GET("/me") () -> Effect[HttpResult[String]] by u: User {
		Ok(u.identity)
	}

	// A state-changing route: the seam also refuses a cross-site request.
	on POST("/rename") () -> Effect[HttpResult[()]] by u: User {
		NoContent
	}
}
```

The cookie defaults to `__Host-session`, `HttpOnly`, `Secure`, `Path=/`, `SameSite=Lax` and no `Domain`,
so it is bound to your origin and unreachable from page scripts. You do not set these; the scheme does.

## Sign in and sign out

Inside the context that declares the actor, `User.issue(identity)` makes the cookie and `User.expire()`
deletes it. Issuing reads the clock, so the handler lists `Clock` in `given`:

```bynk,fragment
on POST("/login") (body: LoginRequest) -> Effect[HttpResult[String]] by v: Visitor given Clock {
  let Ok(id) = UserId.of(body.user) else { BadRequest("bad user") }
  let c <- User.issue(id)
  Created("welcome").withCookie(c)
}

on POST("/logout") () -> Effect[HttpResult[()]] by u: User {
  NoContent.withCookie(User.expire())
}
```

Check the user's credentials before you issue the cookie; the scheme proves *who the cookie was issued
to*, not that they should have it.

## The key

`secret` names a variable, not a key. `bynk deploy` mints a strong key the first time and sets it on the
Worker, keeps it on later deploys, and replaces it only with `--force`
([deploy guide](/book/guides/projects-build-and-deployment/deploy-to-cloudflare/#session-keys)).
`bynk dev` uses a fresh key per run. A key that is missing, not base64url, or under 32 bytes faults the
request rather than letting anyone in.

To **rotate** the key without signing everyone out: declare `previous = "SESSION_KEY_PREV"`, move the old
value there, set the new one in `SESSION_KEY`, deploy, and drop `previous` after one `ttl`. Cookies signed
under either key verify; new ones use the new key.

## What it does and does not protect against

A `SignedCookie` is **signed, not encrypted**: the holder can read the identity inside it, so never
put a secret in an identity. It cannot be revoked before `ttl`: `expire()` clears the browser's copy but
does not invalidate one a user already took, so keep `ttl` short, or keep the session on the server (see
`examples/sessions`) if you must log out everywhere. See
[the reference](/book/reference/actors/#what-a-signed-cookie-does-and-does-not-protect-against) for
the full list.

## Cross-site requests

A state-changing route (`POST`, `PUT`, `PATCH`, `DELETE`) also checks where the request came from. A
browser's own page (`Sec-Fetch-Site: same-origin`) passes. Anything else must carry an `Origin` that is
exactly one of the service's `cors` origins, which must set `credentials: true`; a wildcard or `null`
never counts, and a request that reveals neither is refused with `403`.

## Test it

At the `unit` and `integration` tiers `by User("ada")` gives the handler that identity. At the `system`
tier the driver sends a real signed cookie to the real Worker, and a case can probe the seam:

```bynk,fragment
suite shop.api as system {
	case "a signed-in user is read" {
		let r <- api.GET("/me") by User("ada")
		expect r is Ok("ada")
	}

	case "no cookie is refused" {
		let r <- api.GET("/me") by Nobody
		expect r is Rejected(Unauthorized)
	}

	case "a cross-site post is refused" {
		let r <- api.POST(
			"/rename",
			RequestHeaders.of(["Sec-Fetch-Site: cross-site", "Origin: https://evil.example"])
		) by User("ada")
		expect r is Forbidden
	}
}
```

See [the failure principals](/book/reference/testing/#failure-principals) for `by Forged` and
`by Expired`.
