---
title: Add an authorisation invariant
---
**Goal:** require that a verified user is also *allowed* — an admin-only route —
and keep "not logged in" (`401`) distinct from "logged in but not permitted"
(`403`).

Authentication answers *who you are*; authorisation answers *whether you may*. A
**refinement actor** carves an authorisation invariant out of a base actor:

```bynk
context api

type UserId = String where NonEmpty

actor User { auth = Bearer(secret = "AUTH_JWT_SECRET"), identity = UserId }

actor Admin = User where hasClaim("admin")

service api from http {
	on GET("/admin") () -> Effect[HttpResult[UserId]] by a: Admin {
		Ok(a.identity)
	}
}
```

`Admin` is "a `User` who additionally satisfies the predicate." A handler
`by a: Admin` runs through three steps at the boundary:

1. verify the `User` (Bearer) scheme — failure is **`401`**;
2. check the claim predicate against the **verified** token claims — failure is
   **`403`**;
3. mint the identity and run the body.

The two failures are distinct response channels: the runtime never answers "who
are you" (`401`) when it means "you may not" (`403`).

## The predicate vocabulary

Token claims are untyped, so the `where` predicate is a closed set:

- `hasClaim("name")` — the claim is present and truthy;
- `claimEquals("name", "value")` — the claim equals a string;
- composed with `&&`, `||`, and `!`.

```bynk,fragment
actor Admin = User where hasClaim("admin") && claimEquals("tier", "gold")
```

The claims are an authorisation-time input only: your body still sees just the
sealed identity (`a.identity` is the base `UserId` — an `Admin` *is* a `User`),
so you use an `Admin` anywhere a `User` fits.

## The rules

- The **base must be a `Bearer` actor** — only a token carries claims to test.
- The **predicate must be in the closed set** above.
- A refinement is a handler's sole `by` contract; it is **never a sum member**
  (use it as the whole contract, or narrow inside a
  [resolved arm](/book/guides/actors/multiple-callers/)).

## Testing it

A test acts as the refinement with its base's identity:

```bynk,fragment
case "an admin is answered" {
  let r <- api.GET("/admin") by Admin("root")
  expect r == Ok("root")
}
```

Below the `system` tier the claim is given, not checked. In a `system` suite
the signed token carries claims that satisfy the predicate, so the Worker
admits `by Admin("root")` and answers `by User("root")` with `403`. See
[acting as an actor](/book/reference/testing/#call-site-by).

**See also:** [Reference — Actors](/book/reference/actors/),
[Diagnostic index (`bynk.actor.refinement_*`)](/book/reference/diagnostics/).
