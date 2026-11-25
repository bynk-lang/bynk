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

## Read the claims in the handler

The claim predicate decides whether a request is admitted; it does not hand the handler the claims.
For that, the binder reads the verified token's claims as a `TokenClaims`, for a `Bearer` actor and
for a refinement of one:

```bynk,fragment
on GET("/reports") () -> Effect[HttpResult[String]] by a: Admin {
	let tenant = a.claims.get("tenant").getOrElse("default")
	Ok("\(a.identity) \(tenant)")
}
```

`get(name)` reads a claim that is a JSON string (`None` for any other type, or none at all), and
`has(name)` whether the claim is present. In a test, pass them in the principal:
`by Admin("root", TokenClaims.of(["tenant: acme"]))`. A claim says what the token asserts, so an
object-level check (is this report this tenant's?) stays in the handler. See
[Reading claims](/reference/actors/#reading-claims).

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

A refinement narrows a base actor by a predicate drawn from a closed set for
that base:

| Base | Predicate | Narrows by | In a sum |
|---|---|---|---|
| a `Bearer` actor | `hasClaim("n")`, `claimEquals("n", "v")` | the verified token's claims | never a member |
| `Caller` | `IsContext(a.b, …)` | the calling context | the sum's one `Internal` member |

Both compose with `&&`, `||` and `!`, and a predicate from the other base is
`bynk.actor.refinement_predicate_unsupported`. Any other base is
`bynk.actor.refinement_base_unsupported`.

- A `Bearer` refinement is a handler's sole `by` contract; it is **never a sum
  member** (use it as the whole contract, or narrow inside a
  [resolved arm](/book/guides/actors/multiple-callers/)).
- A refinement over `Caller` restricts which contexts may call a service. It is
  decided at compile time against the project's calls; see
  [Know which context called you](/book/guides/actors/cross-context-callers/#restrict-who-may-call).

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
[acting as an actor](/reference/testing/#call-site-by).

**See also:** [Reference — Actors](/reference/actors/),
[Diagnostic index (`bynk.actor.refinement_*`)](/reference/diagnostics/).
