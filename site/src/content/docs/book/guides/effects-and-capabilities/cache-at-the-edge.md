---
title: Cache responses at Cloudflare's edge
---
**Goal:** serve a route's responses from Cloudflare's edge without running the
Worker, and remove one from the cache when it must change.

`@cache` on its own sets only the `Cache-Control` header. Browsers and other
caches downstream honour it, but Cloudflare doesn't store a response a Worker
generates because of it. Caching at the edge is a separate switch,
`EdgeCache`, which turns on Cloudflare's Workers Cache for the context's
Worker.

`EdgeCache` is **experimental**: it carries no compatibility promise yet. Opt
in in `bynk.toml`:

```toml
[unstable]
allow = ["bynk.cloudflare.EdgeCache"]
```

## Turn the cache on

Declare one `EdgeCache` instance in the context:

```bynk,fragment
context registry.files

consumes bynk.cloudflare { EdgeCache }

capability Edge = EdgeCache
```

The declaration is the switch. The context's Worker gets `[cache] enabled =
true` in its `wrangler.toml`, and a context without an instance is built
exactly as before. A context has one Worker, so it declares at most one
instance, and a bare `given EdgeCache` is refused.

## Mark what may be cached

A route opts in with its ordinary `@cache`, and `scope: public` is what lets
Cloudflare share the response between callers:

```bynk,fragment
service files from http {
	@cache(maxAge: 365.days, scope: public, immutable: true)
	on GET("/dl/sha256/:hex") (hex: String) -> Effect[HttpResult[()]] by v: Visitor {
		RawTagged(Bytes.fromUtf8("archive \(hex)"), "application/octet-stream", "\"sha256-\(hex)\"")
	}
}
```

Once the cache is on, the Worker sends:

| Response | `Cache-Control` sent |
|---|---|
| a `2xx` from a `GET` with `@cache` | the route's |
| any other response to a `GET` or `HEAD` | `no-store` |
| a `304` | copied from the `200` it stands for |
| any other method | unchanged |

So only what you marked is kept. A `404` from the archive route is `no-store`,
so a missing archive is never cached, even briefly. A `GET` route without
`@cache` is `no-store` too, and so are the router's own `404` and `405` answers
and a fault's `500`.

`scope: private` is never shared. `scope: public` is shared with everyone,
even a response sent to a request that carried `Authorization`: `public` allows
that (RFC 9111 §3.5). In a context with
`EdgeCache`, `scope: public` means "shared at Cloudflare's edge", so use it
only for a response that is the same for every caller. `bynk check` refuses
it on a route whose actor authenticates (a `Bearer` or `Signature` actor, say).
A route that answers in the caller's language is fine: a Worker that
negotiates its `Locale` sends `Vary: accept-language`, and the edge keeps one
response per language.

## Purge after a change

`immutable: true` promises the bytes at a URL never change, and a cache holds
them for the whole `max-age` on that promise. A takedown breaks it, so a route
that can ever change needs a way to remove what was cached. That is
`purge`:

```bynk,fragment
service operator from http {
	on POST("/takedown/:hex") (hex: String) -> Effect[HttpResult[Bool]] by v: Visitor given Edge {
		let purged <- Edge.purge(["/dl/sha256/\(hex)"])
		Ok(purged)
	}
}
```

Each argument is a path prefix. It starts with `/` and has no query or
fragment. Every cached response under it is removed, everywhere. A
content-addressed path has a fixed length, so its prefix names exactly one
response.

`purge` gives `false` when Cloudflare refuses the purge. Purges are
rate-limited at Cloudflare's Free-tier limits on every plan, so treat `false` as
"not done yet": report it, and run the purge again. Purge on a takedown, not on
every publish. For a file that changes often, such as an index, use a short
`maxAge` instead.

A purge reaches the cache only from the Worker's own `fetch`. Call it from a
service handler, or a provider one uses. Never call it from an agent or
`on schedule`. Cloudflare accepts a purge from a cron run, but it clears
nothing, so `bynk check` refuses it.
An agent runs in a Durable Object, so `bynk check` refuses an agent handler
that reaches `Edge`. Purge from the route that calls the agent instead. A queue
consumer is refused too, and so is exporting a capability whose provider uses
`Edge`. Another context purges by calling one of this context's services.

## Test it

In a test there is no edge cache. Each instance's double accepts every purge,
and observation checks what was purged:

```bynk,ignore
case "a takedown purges the archive's path" {
	let purged <- takedown.call("abc")
	expect purged
	expect Edge.purge called once with prefixes == ["/dl/sha256/abc"]
}

case "a refused purge is reported" {
	stub Edge.purge(_) returns false
	let purged <- takedown.call("abc")
	expect !purged
}
```

## Run it locally

`bynk dev` runs the Worker under `wrangler dev`, which has no edge cache: every
request runs the Worker, and the headers above are still sent. `purge` gives
`true` and logs `bynk: EdgeCache.purge is a no-op under bynk dev`. To see a
response served from the edge, deploy it and fetch the same URL twice: the
second answer carries `cf-cache-status: HIT`.

## Deploy it

Nothing is created on the account. Workers Cache has no namespace to allocate,
so `bynk deploy` uploads the `[cache]` setting with the Worker, and nothing is
added to `bynk.deploy.lock`.

## Related

- [`EdgeCache` reference](/book/reference/bynk-capabilities/#edge-caching-edgecache)
- [HTTP reference: declared freshness (`@cache`)](/book/reference/http/#declared-freshness-cache)
