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

See [acting as an actor](/book/reference/testing/#call-site-by).

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
the [reference](/book/reference/actors/).
