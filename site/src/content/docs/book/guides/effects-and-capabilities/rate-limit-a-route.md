---
title: Rate-limit a route
---
**Goal:** refuse a caller who calls a route too often, with a `429`, using
Cloudflare's Rate Limiting binding.

`RateLimit` is **experimental**: it carries no compatibility promise yet. Opt in
in `bynk.toml`:

```toml
[unstable]
allow = ["bynk.cloudflare.RateLimit"]
```

## Declare a limiter

A limiter is a named instance of `RateLimit` that sets how many calls a key may
make (`@limit`) in each window (`@period`, `10.seconds` or `60.seconds`):

```bynk,fragment
context registry.publish

consumes bynk.cloudflare
consumes bynk.cloudflare { RateLimit }

capability PublishLimit = RateLimit @limit(20) @period(60.seconds)
```

A bare `given RateLimit` is refused: it would set neither. Declare one instance
per limit you need. Two limiters never share counts, so a per-token publish
limit and a per-route read limit are two declarations.

## Check it in the handler

`allow(key)` counts one call for `key` and gives `false` once that key is over
its limit for the window. The key is whatever identifies the caller to you: a
token, an account, a route. Return `TooManyRequests` when it is over:

```bynk,fragment
service api from http {
	on PUT("/api/:token") (token: String) -> Effect[HttpResult[String]] by Visitor given PublishLimit {
		let allowed <- PublishLimit.allow(token)
		if allowed {
			Ok("published")
		} else {
			TooManyRequests("slow down: 20 publishes a minute per token")
		}
	}
}
```

Limiting by the caller's IP address needs the handler to read a request header
(`CF-Connecting-IP`), which handlers can't do yet; limit by a value the handler
already has.

## Know what it promises

The count is **approximate**. Cloudflare keeps it per location and it is
eventually consistent, so a key can get somewhat more than its limit,
especially when its calls arrive at many locations at once. That is the right
trade for abuse and cost control: the check is fast and needs no storage.

It is the wrong tool for an exact quota, such as a billing limit or a hard
per-account cap. Use an agent, one per key, as the
[rate limiter example](/by-example/projects/rate-limiter/) does: it is exact,
at the cost of a Durable Object call per request.

## Test it

In a test each limiter counts calls per key in memory, allowing its `@limit`
per `@period`, on the [harness clock](/book/reference/testing/#time-in-tests):

```bynk,ignore
case "a token is refused after twenty publishes" {
	// … twenty calls that are allowed …
	let refused <- publish.call("t")
	expect !refused
	advance Clock by 60.seconds
	let again <- publish.call("t")
	expect again
}
```

To test the refusal path without counting, stub it:
`stub PublishLimit.allow(_) returns false`.

## Deploy it

`bynk deploy` gives each limiter a `namespace_id`, records it in
`bynk.deploy.lock`, and keeps it across deploys, including when you change
`@limit` or `@period`. Nothing is created on the account. See
[Deploy to Cloudflare](/book/guides/projects-build-and-deployment/deploy-to-cloudflare/#rate-limiters).

## Related

- [`RateLimit` reference](/book/reference/bynk-capabilities/#rate-limiting-ratelimit)
- [HTTP reference: `TooManyRequests`](/book/reference/http/#4xx-client-error)
