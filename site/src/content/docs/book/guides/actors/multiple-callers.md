---
title: Serve several kinds of caller from one route
---
**Goal:** answer one route for more than one kind of party — a richer view for a
signed-in user, a public view for everyone else — without splitting the route or
hand-rolling the "try auth A, else B" branching.

Name an **ordered sum of peer actors** on the `by` clause with `|`. The boundary
tries each in declared order and binds the **first** that verifies; the body
`match`es on which one it was.

```bynk
context api

type UserId = String where NonEmpty

type Note = {
	id: String,
	owner: String,
}

actor User { auth = Bearer(secret = "AUTH_JWT_SECRET"), identity = UserId }

service api from http {
	on GET("/notes/:id") (id: String) -> Effect[HttpResult[Note]] by who: User | Visitor {
		match who {
			User(u) => Ok(Note { id: id, owner: u }),
			Visitor => Ok(Note { id: id, owner: "public" }),
		}
	}
}
```

Each arm binds that actor's identity **directly** — `User(u)` gives `u : UserId`
(the arm already names the actor, so there is no `.identity` step); a party with
no identity, like `Visitor`, binds nothing. If **no** member verifies, the route
fails closed with `401`.

## The rules

A sum is checked for reachability — decidably, at the scheme level:

- **It needs a binder** — the body learns which party verified by matching it.
- **Peers are distinguished by scheme**, so no two members may share one (`User |
  Visitor` ✓; two `Bearer` actors ✗).
- **A catch-all comes last.** `Visitor` (scheme `None`) accepts everyone, so
  anything after it is unreachable — write `User | Visitor`, never `Visitor |
  User`.
- **Refinements are not members.** `User | Admin` is rejected — every `Admin` is
  a `User`, so the arm is dead. Narrow *inside* an arm instead (see
  [authorisation invariants](/book/guides/actors/authorisation/)).
- The body `match` must be **exhaustive** over the members.

## Testing each arm

A test drives each arm by naming that member, with the identity it carries:

```bynk,fragment
case "a visitor sees the public view" {
  let r <- api.GET("/notes/:id", "n1") by Visitor
  expect r is Ok(_)
}

case "a user sees their own view" {
  let r <- api.GET("/notes/:id", "n1") by User("ada")
  expect r is Ok(_)
}
```

A bad credential is tested the same way, by naming the member whose refusal the case checks. A sum can hold
several members that refuse a forged credential, so `by Forged` names one in parentheses:

```bynk,fragment
case "a forged session cookie is refused" {
  let r <- api.GET("/orders") by Forged(User)
  expect r is Rejected(Unauthorized)
}
```

A bare `by Forged` is accepted only when exactly one member can refuse it, and then it is presented to that
member; when more than one could, the compiler reports `bynk.test.bad_principal_needs_arm` and asks you to name
the member. `by Expired` and
`by Stale` work the same way, and `by Revoked(User)` tests a `sessions = true` cookie. A bad credential takes no
identity.

See [acting as an actor](/reference/testing/#call-site-by).

## A browser session or an API token

The most common real arrangement for an API is a web app signed in with a session cookie, plus CLI,
mobile or partner clients sending a token, all against the same routes. Declare both and name them in one
`by` clause:

```bynk
context shop.api

type UserId = String where NonEmpty

actor User { auth = SignedCookie(secret = "SESSION_KEY", ttl = 12.hours), identity = UserId }

actor ApiClient {
	auth = Oidc(
		issuer = "https://id.example",
		audience = "api",
		jwks = "https://id.example/jwks"
	),
	identity = UserId
}

service api from http {
	cors {
		origins: ["https://app.example.com"],
		credentials: true,
	}

	on POST("/orders") () -> Effect[HttpResult[String]] by who: ApiClient | User {
		match who {
			ApiClient(c) => Ok("token \(c)"),
			User(u) => Ok("cookie \(u)"),
		}
	}
}
```

The members are tried in the order written. The cookie's cross-site check belongs to the cookie arm: it
runs only when a session cookie is present, so a token client never meets it, while a cross-site page that
sends only the user's cookie is refused (`403`). A request carrying both a cookie and a token is resolved
by the order, which is why the example lists the token first: an explicit token wins over an ambient
cookie. A test acts as either member (`by User("ada")`, `by ApiClient("ci")`), and `by Nobody` is the
request with no credential, which is `401`.

## Mixing a header and a body member

Members can verify different ways — a header (`Bearer`) and a body
(`Signature`) — in one route. The boundary reads the body once, tries each
member against the material in hand, and parses the body from the same bytes:

```bynk
context api

type UserId = String where NonEmpty

type Event = {
	id: String,
}

actor User { auth = Bearer(secret = "AUTH_JWT_SECRET"), identity = UserId }

actor Hook { auth = Signature(secret = "WH_SECRET", header = "X-Signature") }

service api from http {
	on POST("/ingest") (body: Event) -> Effect[HttpResult[String]] by who: User | Hook {
		match who {
			User(u) => Ok(u),
			Hook => Ok(body.id),
		}
	}
}
```

**Next:** [add an authorisation invariant](/book/guides/actors/authorisation/) to a member, or read
the [reference](/reference/actors/).
