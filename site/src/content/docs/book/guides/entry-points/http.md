---
title: "Handle an HTTP request and shape an `HttpResult`"
---
**Goal:** answer an HTTP request, reading path parameters and a request body, and
return the right status.

Handlers go in a `service` inside a `context`. Each handler names a verb, a route,
its parameters, and returns `Effect[HttpResult[T]]`.

## A handler with no input

```bynk
context notes

service api from http {
	on GET("/ping") () -> Effect[HttpResult[String]] by Visitor {
		Ok("pong")
	}
}
```

`Ok("pong")` is the `HttpResult` variant for `200 OK`.

## Read a path parameter

A `:name` segment in the route becomes a parameter of the same name:

```bynk,fragment
  on GET("/notes/:id") (id: String) -> Effect[HttpResult[String]] by Visitor {
    NotFound
  }
```

A parameter is not limited to text. It takes the types a typed query field
takes (`Int`, `Bool`, `Decimal`, an enum, or a refinement over one of those), so
an order number can be a positive `Int`, read and checked before the handler runs:

```bynk,fragment
type OrderNo = Int where Positive
```

```bynk,fragment
  on GET("/orders/:id") (id: OrderNo) -> Effect[HttpResult[String]] by Visitor {
    Ok("order \(id)")
  }
```

`/orders/42` reaches the handler with `id` bound to `42`. `/orders/abc` is a `400`
`StructuralMismatch` and `/orders/0` a `400` `RefinementViolation`, both at
`path.id`, so the handler never re-checks. The route matched, so neither is a `404`.

## Accept a date in a URL

A path parameter of a refined type is checked before the handler runs.
`IsoDate` refines a `String` to a calendar date, exactly `YYYY-MM-DD`, that
exists:

```bynk,fragment
type Day = String where IsoDate
```

```bynk,fragment
  on GET("/digest/:day") (day: Day) -> Effect[HttpResult[String]] by Visitor {
    Ok(day)
  }
```

`/digest/2026-02-30`, `/digest/2026-13-01` and `/digest/yesterday` are a `400`
`RefinementViolation` at the boundary, so the handler never re-checks the
calendar. The same rule holds for a body field, `Day.of(…)` and `Json.decode`,
and a string literal such as `"2026-11-03"` is checked at compile time.

A `Day` is still a `String`, so it interpolates and stores as one. To reach the
day's first instant, pair it with `Instant.fromIso`, which is always `Some` for
a `Day`; add `+ 1.day` for the next day:

```bynk,fragment
let start = Instant.fromIso("\(day)T00:00:00Z")
```

## Filter and paginate a `GET` with a typed query

A `query` parameter whose type is a record reads the query string. The router
decodes it, applies the defaults and checks the refinements before the handler
runs, so the handler only ever sees a valid filter:

```bynk,fragment
type Limit = Int where InRange(1, 500)

type Filter = {
  owner: Option[String],
  limit: Limit = 50,
}

service api from http {
  on GET("/notes") (query: Filter) -> Effect[HttpResult[String]] by Visitor {
    Ok("up to \(query.limit)")
  }
}
```

`GET /notes?owner=ada&limit=100` reaches the handler; `GET /notes?limit=0` is a
`400` before it. Keys the record doesn't declare are ignored. For keys you can't
name ahead of time, take a raw [`query: RequestQuery`](/reference/http/#request-query)
instead. See [Query strings](/reference/http/#query-strings).

## Accept a request body

A `body` parameter is parsed and validated from the request's JSON before the
handler runs — an invalid body is rejected with `400` at the boundary:

```bynk
type NewNote = { title: String }

service api from http {
  on POST("/notes") (body: NewNote) -> Effect[HttpResult[NewNote]] by Visitor {
    Created(body)
  }
}
```

A JSON route answers `415` to a form or multipart post, so a `curl` call names its
type:

```sh
curl -X POST http://localhost:8787/notes \
  -H 'content-type: application/json' \
  -d '{"title": "first"}'
```

## Choose the right status

Return the `HttpResult` variant matching the outcome — `Ok` (200),
`Created` (201), `Accepted` (202), `NoContent` (204), a redirect such as
`Found(url)` (302) or `SeeOther(url)` (303), `BadRequest(msg)` (400),
`Unauthorized` (401), `Forbidden` (403), `NotFound` (404), `Conflict(msg)` (409),
`UnprocessableEntity(msg)` (422), `TooManyRequests(msg)` (429),
`ServerError(msg)` (500), `ServiceUnavailable(msg)` (503). See the
[HTTP reference](/reference/http/) for the full table. Map domain errors
to statuses with `match`:

```bynk
fn handle(ok: Bool) -> HttpResult[String] {
  if ok {
    Ok("done")
  } else {
    BadRequest("bad input")
  }
}
```

## Answer a missing header with `400`

`?` on an `Option` answers `404 NotFound`, which is right for a lookup that
misses and wrong for a request that left out a required header. Name the status
with `let … else`:

```bynk,fragment
on POST("/hook") (headers: RequestHeaders, body: Bytes) -> Effect[HttpResult[String]] by Sender {
  let Some(event) = headers.get("X-Event") else { BadRequest("missing X-Event") }
  Ok(event)
}
```

The checker warns on `headers.get("X")?` and offers this rewrite as a fix. See
[HTTP → Answering another status](/reference/http/#answering-another-status).

## Stream an incremental response

To send a response as it is produced — a progress feed, a token relay, a live
tick — return `Streaming(stream)` instead of a buffered `Ok(value)`. It carries a
[`Stream[String]`](/reference/types/#stream) and emits it as Server-Sent
Events (`text/event-stream`); each stream element is one `data:` event.

```bynk
context feed

service Feed from http {
	on GET("/ticks") () -> Effect[HttpResult[()]] by Visitor {
		Streaming(Stream.of(["tick-1", "tick-2", "tick-3"]).take(3))
	}
}
```

A streamed response returns `Effect[HttpResult[()]]` — there is no JSON body
value, so the parameter is `()`. Because the status is sent before the first
chunk, streaming is **200-only**: decide any failure *before* you start, and
return an ordinary variant instead — it shares `HttpResult[()]`, so both live in
one handler:

```bynk,fragment
on GET("/feed/:mode") (mode: String) -> Effect[HttpResult[()]] by Visitor {
  if mode == "live" {
    Streaming(Stream.of(["a", "b", "c"]).take(2))
  } else {
    NotFound
  }
}
```

See [HTTP → Streamed responses](/reference/http/#streamed-responses) for
the framing rules and the mid-stream-error pattern.

## Stream events a browser can resume

A browser's `EventSource` reconnects when a stream drops and sends the last `id` it
saw as `Last-Event-ID`. To give it something to resume from, stream `SseEvent`
records (from `bynk`) instead of plain strings: `id` names the event, `event` picks
the name it is dispatched under, and `retry` sets the reconnection delay.

```bynk
context updates

consumes bynk { SseEvent }

service Updates from http {
	on GET("/updates") () -> Effect[HttpResult[()]] by Visitor {
		Streaming(Stream.of([
			SseEvent {
				data: "first",
				id: Some("1"),
				event: Some("update"),
			},
			SseEvent {
				data: "second",
				id: Some("2"),
				event: Some("update"),
				retry: Some(5.seconds),
			},
		]))
	}
}
```

Only `data` is required. See
[HTTP → Structured events](/reference/http/#structured-events) for the framing and the
values the wire cannot carry.

## Return a non-JSON body

Handlers return typed values, serialised as JSON. To serve something that is
*not* JSON — `robots.txt`, `sitemap.xml`, an RSS feed, a CSV download, a QR-code
PNG — return `Raw(body, contentType)`. It writes a raw
[`Bytes`](/reference/types/#bytes) body straight into the response under the
`content-type` you declare, with **no codec** in between.

`Bytes` is binary-first: a PNG flows in directly, and text goes through
`Bytes.fromUtf8`, which makes the charset explicit — the body is UTF-8, so pair
it with a matching `content-type`.

```bynk
context site

service Site from http {
	on GET("/sitemap.xml") () -> Effect[HttpResult[()]] by Visitor {
		let xml = "<?xml version=\"1.0\"?><urlset></urlset>"
		Raw(Bytes.fromUtf8(xml), "application/xml")
	}
}
```

Like `Streaming`, `Raw` returns `Effect[HttpResult[()]]` and is **200-only** —
it is for service-tier bodies, which are almost always `200`.

A body too large to hold in memory streams instead: `RawStreamed(body,
contentType)` takes a `Stream[Bytes]`, usually from a capability, and writes each
chunk as the client reads it. It sends no `ETag`, and a fault partway through cuts
the response off, since the status is already sent. See
[Streamed raw responses](/reference/http/#streamed-raw-responses).

**Why can't I just return HTML?** Because rendering is the frontend tier's job,
not the service's. Bynk serves *bytes with a content-type*; it deliberately has
no HTML template layer. A page — including a styled `404` — belongs in the
frontend (Cloudflare Pages), not in a handler. See
[HTTP → Raw responses](/reference/http/#raw-responses) for the full rules.

## Call it from a browser (CORS)

By default a `from http` service is **same-origin**: a browser page on a
*different* origin cannot read its responses, because the response carries no
`Access-Control-*` headers and there is no `OPTIONS` handler for the browser's
preflight. That is the safe default — you opt a service in, per origin.

To make a service cross-origin callable, add a `cors { }` policy at the top of the
service body and list the origins that may call it:

```bynk
context api

service api from http {
	cors {
		origins: ["https://app.example.com"],
	}

	on GET("/items/:id") (id: String) -> Effect[HttpResult[String]] by v: Visitor {
		Ok(id)
	}
}
```

That is all it takes. From the policy the compiler:

- answers the browser's **preflight** — an `OPTIONS` to any of the service's
  routes returns `204` with the `Access-Control-*` headers. The preflight is
  answered *before* the handler's `by`/Bearer authentication, because a browser
  sends it with no credentials attached — so a preflight is never rejected by an
  actor check; and
- **stamps `Access-Control-Allow-Origin`** on every response the service returns.

The allowed **methods** are taken from the routes you already declared — you never
restate them. Allowed **headers** default to `content-type` (plus `Authorization`
when the service authenticates with a Bearer actor). Add `credentials: true` to
allow cookies / `Authorization`, and `maxAge: 1.hour` to let the browser cache
the preflight. A request from an origin *not* on the list simply gets no grant, so
the browser blocks it — the same fail-closed posture as authentication.

> **Wildcard + credentials is a compile error.** `origins: ["*"]` opens the service
> to any origin, but you cannot combine it with `credentials: true` — the browser
> rejects that pair at runtime, so Bynk rejects it at compile time. With
> credentials, name the exact origins.

See [HTTP → CORS](/reference/http/#cors) for the full field table and matching
rules.

### Cross-site cookies need `credentials: true`

A cookie set with `sameSite: SameSite.Unrestricted` is meant to travel on cross-site
requests, but a browser sends cookies on a cross-origin `fetch` only when the request is
credentialed *and* the service answers with `credentials: true`. A service that attaches
such a cookie without that policy gets a warning
(`bynk.http.cookie_cross_site_without_credentials`), because the cookie could never come
back:

```bynk,fragment
service api from http {
  cors { origins: ["https://app.example.com"], credentials: true }

  on POST("/login") () -> Effect[HttpResult[String]] by v: Visitor {
    Created("ok").withCookie(Cookie { name: "sid", value: "a", sameSite: SameSite.Unrestricted })
  }
}
```

Likewise, a `domain` on a cookie makes every subdomain receive it, so it warns
(`bynk.http.cookie_domain_widens`) unless the name starts `__Host-`, where a `domain` is an
error. Drop `domain` to keep a cookie to the host that set it. See
[HTTP → Setting cookies](/reference/http/#setting-cookies).

## Set a cookie from the record that reads it

A cookie that a typed `cookies` record reads is written from that record, so its name and
its value cannot drift from what the next request decodes:

```bynk,fragment
type Theme = enum { Light, Dark }
type Prefs = {
  theme: Theme = Light,
  @name("__Host-visits") visits: Int = 0,
}
```

```bynk,fragment
on GET("/visit") (cookies: Prefs) -> Effect[HttpResult[String]] by v: Visitor {
  let n = cookies.visits + 1
  Ok("visit \(n)").withCookie(Prefs.visits.cookie(n))
}

on POST("/theme/dark") () -> Effect[HttpResult[String]] by v: Visitor {
  Ok("dark").withCookie(Cookie { ...Prefs.theme.cookie(Dark), maxAge: Some(365.days) })
}

on POST("/theme/reset") () -> Effect[HttpResult[()]] by v: Visitor {
  NoContent.withCookie(Prefs.theme.expire())
}
```

`Prefs.theme.cookie(Dark)` is a `Cookie` named by the field's wire name (`theme`; for `visits`
it is the `@name`, `__Host-visits`) and valued as the decoder reads it. Writing `dark` by hand
in a literal `Cookie` would make every later request a `400`, because the decoder reads an
enum by its exact variant name, so the compiler rejects it
(`bynk.http.cookie_value_undecodable`). A literal `Cookie` that a record reads, even with a
decodable value, draws a warning (`bynk.http.cookie_untyped_has_field`). Spread the typed cookie into a `Cookie { … }` literal
for attributes (`maxAge`, `domain`, `path`, `sameSite`), and give `expire()` the same `Path`
and `Domain` the cookie was set with. A cookie in no record is still written as a plain
`Cookie { name, value }`.

Decoding a cookie is not authenticating it: a `cookies` record reads what the browser sends,
and anyone can send any value. To sign a user in, use a [signed cookie](/book/guides/actors/authenticate-with-a-signed-cookie/)
(its session cookie cannot be written through a record). See
[HTTP → Writing a cookie from its record](/reference/http/#typed-cookie-write).

## Add a header to a response

A header the handler owns goes on any `HttpResult` with `withHeader`. The statuses that have a
standard header take a typed helper instead, which checks the value for you:

```bynk,fragment
on GET("/report") () -> Effect[HttpResult[String]] by v: Visitor {
  Ok("ready").withHeader("X-Request-Id", "abc")
}

on POST("/things") (body: String) -> Effect[HttpResult[String]] by v: Visitor {
  Created(body).at("/things/1")
}

on GET("/search") () -> Effect[HttpResult[String]] by v: Visitor {
  TooManyRequests("slow down").retryAfter(30.seconds)
}
```

`.at(url)` writes `Location` on a `Created`, `.retryAfter(delay)` writes `Retry-After` in whole
seconds on a `TooManyRequests` or `ServiceUnavailable`, and `.download(filename)` writes
`Content-Disposition: attachment` on a `Raw` result. Write a helper on the variant itself, and
call it once: a second call replaces the first.

`withHeader` refuses the headers the router writes itself (`Cache-Control`, `Set-Cookie`,
`Content-Type`, `Location`, the security and CORS headers and the rest), because setting them here
would undo `@cache`, `security { }` or `cors { }`. A literal that is one, or that is not a valid
header, is a compile error (`bynk.http.reserved_response_header`,
`bynk.http.invalid_response_header`); a computed one answers `500` and is logged. In a test,
`result.headers()` reads the headers a result carries. See
[HTTP → Response headers](/reference/http/#response-headers).

## Secure by default (security headers)

You don't have to do anything to get the one security header that matters for a
data API: every `from http` response already carries
`X-Content-Type-Options: nosniff`, which stops a browser from MIME-sniffing a JSON
or text body into HTML and running it. It has no downside, so it is on by default
— a security header you have to remember to switch on is the one you forget.

The one header with a real footgun is opt-in. `Strict-Transport-Security` (HSTS)
pins a browser to HTTPS for its whole lifetime — great in production, but it
breaks a custom domain served over plain HTTP in dev, and it is hard to undo once
a browser has cached it. So you turn it on deliberately, with `hsts`:

```bynk
context api

service api from http {
	security {
		hsts: 180.days,
	}

	on GET("/items/:id") (id: String) -> Effect[HttpResult[String]] by v: Visitor {
		Ok(id)
	}
}
```

Reach for `security { nosniff: false }` only if you have a specific reason to send
no security headers (for instance, HSTS and `nosniff` are already terminated at
your edge). See [HTTP → Security headers](/reference/http/#security-headers)
for the field table and the caveats.

## Cache a read endpoint

A read endpoint that returns the same bytes to repeated callers is wasting
bandwidth. Two things fix that, and Bynk splits them by who knows what.

**Revalidation is automatic.** Every `GET` that returns `Ok` already carries a weak
`ETag` over its body — you write nothing. A browser that re-requests with the
`ETag` it saved gets a `304 Not Modified` with an empty body instead of the whole
payload:

```bynk,fragment
on GET("/links/:code") (code: String) -> Effect[HttpResult[String]] by v: Visitor {
  Ok(code)
}
```

`GET /links/abc` → `200` + `ETag: W/"…"`. The next `GET /links/abc` with
`If-None-Match: W/"…"` → `304`, empty body. Nothing to configure.

**Freshness you declare.** Only you know whether a client may serve a response
*without* checking back, and for how long. Say so with `@cache`, written just above
the handler:

```bynk,fragment
@cache(maxAge: 5.minutes)
on GET("/links/:code") (code: String) -> Effect[HttpResult[String]] by v: Visitor {
  Ok(code)
}
```

That adds `Cache-Control: private, max-age=300`. Reach for `scope: public` only
when a **shared** cache or CDN should store the response too:

```bynk,fragment
@cache(maxAge: 1.hour, scope: public)
on GET("/config") () -> Effect[HttpResult[String]] by v: Visitor {
  Ok("…")
}
```

The default is `private` — a per-user cache, never a shared one — because that is
the safe choice for anything that might vary by caller. A duration's unit agrees with
its number (`5.minutes`, `1.hour`), and `@cache` is only valid on a `GET` returning `Ok`
(the one exception is `noStore`, below); the compiler flags a bare number or a misplaced
annotation.

**A route guarded by a credential is private without asking.** If the route's actor is a
`Bearer`, `Oidc`, `SignedCookie` or `Signature` one, its responses are one caller's, so they
carry `Cache-Control: private` whether or not you write `@cache` — a `200`, but also a `404`
or the actor's own `401`. The caller's browser can still keep and revalidate them; only a
shared cache is told not to store them. `scope: public` on such a route is refused, because
a CDN would replay one caller's answer to another.

For data that must never be stored at all, such as a one-time token, say so with `noStore`.
It works on a route of any HTTP method, and it takes no `maxAge`:

```bynk,fragment
@cache(noStore: true)
on GET("/me/tokens") () -> Effect[HttpResult[String]] by v: Visitor {
  Ok("…")
}
```

That sends `Cache-Control: no-store` and no `ETag`, since nothing is stored to revalidate.

See [HTTP → Caching](/reference/http/#caching) for the eligibility rules and
the full field table.

## Cap request size

An endpoint that takes a body should say how big a body it will accept — otherwise
a client can stream an arbitrarily large payload before you can turn it away. Set a
service-wide default with `limits { }`, and override it for the one route that
needs a bigger ceiling with `@limit`:

```bynk
context uploads

service uploads from http {
  limits {
    maxBody: 1_048_576,        // 1 MiB — the default for every body-taking route
  }

  @limit(maxBody: 26_214_400)  // 25 MiB — this one endpoint accepts a larger upload
  on POST("/files") (body: String) -> Effect[HttpResult[String]] by v: Visitor {
    Ok("stored")
  }

  on PATCH("/files/:code") (code: String, body: String) -> Effect[HttpResult[String]] by v: Visitor {
    Ok(code)
  }
}
```

`POST /files` caps at 25 MiB (its `@limit` wins); `PATCH /files/:code` falls back
to the 1 MiB service default. A request whose `Content-Length` exceeds the cap is
rejected with a `413 PayloadTooLarge` *before the body is read*, and so is a body
that runs past the cap as it is read (a chunked one, or one whose length is
understated) — nothing reaches your handler. `@limit` is only valid on a body-taking method (`POST`/`PUT`/`PATCH`);
`maxBody` is a byte count (the `_` is just visual grouping). A route with no cap is
unchanged, so this is opt-in.

See [HTTP → Request body limits](/reference/http/#request-body-limits) for the
precedence rule, how the bytes read are counted, and the field tables.

## Serve JSON to non-Bynk clients

Bynk writes an `Option` in a response body as `{"kind": "Some", "value": …}`, which
every non-Bynk consumer then has to special-case. Opt the service into plain JSON,
so `Some(v)` is `v` and `None` is `null`:

```bynk
context activity

type Activity = { id: String, action: Option[String], deleted: Option[Bool] }

service api from http {
  json { options: plain }

  on GET("/activity/:id") (id: String) -> Effect[HttpResult[Activity]] by r: Visitor {
    Ok(Activity { id: id, action: Some("opened"), deleted: None })
  }

  @json(options: tagged)       // this one route keeps Bynk's tagged form
  on GET("/activity/:id/raw") (id: String) -> Effect[HttpResult[Option[String]]] by r: Visitor {
    Ok(Some("x"))
  }
}
```

`GET /activity/1` answers `{"id":"1","action":"opened","deleted":null}`. A Bynk
client still reads it. Plain mode refuses an `Option[Option[T]]` or `Option[()]` in a response
type (`Some(None)` and `None` would both be `null`).

See [HTTP → JSON for non-Bynk clients](/reference/http/#json-for-non-bynk-clients).

## Build and run

HTTP services compile to a Cloudflare Worker with `--target workers`. See
[Target Cloudflare Workers](/book/guides/projects-build-and-deployment/cloudflare-workers/).

## Related

- Tutorial: [Build a small HTTP service](/book/tutorials/http-service/).
- Reference: [HTTP](/reference/http/) — the complete variant/status table.
