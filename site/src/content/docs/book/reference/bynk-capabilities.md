---
title: "First-party `bynk` capabilities"
---
Bynk ships a small set of capabilities with the compiler, under the reserved
`bynk` namespace. You consume them like any capability — `consumes` the unit,
then `given` the capability in the handlers that need it — but you never declare
or provide them: the toolchain injects the implementation for the target
platform. The `bynk` root namespace is reserved, so your own code can never
collide with it.

There are three units:

| Unit | Consume with | Portability |
|---|---|---|
| **`bynk`** | `consumes bynk { … }` | Portable — the same source runs on the `cloudflare` and `node` platforms. |
| **`bynk.cloudflare`** | `consumes bynk.cloudflare { … }` | Platform-locked — consuming it pins the deployment unit to Cloudflare. |
| **`bynk.crypto`** *(experimental)* | `consumes bynk.crypto { … }` | Portable — `cloudflare`, `node` and `browser`, over Web Crypto. |

For *why* a capability is the unit of outside-world access, see
[Understand the capability model](/book/guides/effects-and-capabilities/understand-the-capability-model/);
for the general `capability` / `provides` / `given` rules, see
[Capabilities & providers](/book/reference/capabilities/).

## The portable surface — `bynk`

`consumes bynk { … }` brings these into scope. They are implemented identically
on every platform (the host `Date.now`, `crypto`, `fetch`, `console`,
environment), so code that stays on this surface is portable.

| Capability | Operations |
|---|---|
| **`Clock`** | `now() -> Effect[Instant]` — the current instant (an absolute point in time; epoch-millis under the hood). |
| **`Random`** | `uuid() -> Effect[Uuid]` · `int(lo: Int, hi: Int) -> Effect[Int]` (lo-inclusive, hi-exclusive) · `bytes(n: Int) -> Effect[Bytes]` — `n` random bytes. `int` is **not** for secrets; use `bytes`. |
| **`Logger`** | `info(msg: String) -> Effect[()]` · `error(msg: String) -> Effect[()]` · `warn(msg: String) -> Effect[()]` · `debug(msg: String) -> Effect[()]` · `log(level: LogLevel, msg: String, fields: Map[String, String]) -> Effect[()]` — a structured record (experimental). See [Log levels and structured fields](#log-levels-and-structured-fields). |
| **`Fetch`** | `send(req: Request) -> Effect[Result[Response, FetchError]]` — an outbound HTTP request · `sendWith(req: Request, options: FetchOptions) -> Effect[Result[Response, FetchError]]` — with a timeout and a redirect policy (experimental) · `receiveBytes(req: Request, options: FetchOptions) -> Effect[Result[BytesResponse, FetchError]]` · `sendBytes(req: Request, body: Bytes, options: FetchOptions) -> Effect[Result[BytesResponse, FetchError]]` — binary bodies (experimental) · `receiveStream(req: Request, options: FetchOptions) -> Effect[Result[StreamResponse, FetchError]]` · `sendStream(req: Request, body: Stream[Bytes], options: FetchOptions) -> Effect[Result[StreamResponse, FetchError]]` — streamed bodies (experimental). See [Timeouts and redirects](#timeouts-and-redirects), [Binary bodies](#binary-bodies) and [Streamed bodies](#streamed-bodies). |
| **`Secrets`** | `get(name: String) -> Effect[Option[String]]` — read configuration/secrets; `None` if unset. |
| **`Locale`** | `current() -> Effect[LocaleTag]` — the locale to render messages in. On Cloudflare, negotiated from the request's `Accept-Language` against a context's message bundle; a fixed `"en"` on every other platform, and on Cloudflare without a detectable bundle. See [Understand localisation](/book/guides/localisation/understand-localisation/). |
| **`Idempotency`** | `dedup[T](key: String) -> Effect[Option[T]]` · `remember[T](key: String, value: T, expiresAfter: Duration) -> Effect[()]` · `claim(key: String, expiresAfter: Duration) -> Effect[Claim]` — mechanical dedup for at-least-once delivery. See below. |
| **`Events`** | `emit[E](event: E) -> Effect[()]` — emit an `event` declared by the calling context, fire-and-forget. See below. |

`Fetch` sends a request over the public internet. On Cloudflare that holds even
when the URL is another Worker on the same account, such as a sibling Bynk
service's `workers.dev` address, because every emitted Worker carries the
`global_fetch_strictly_public` compatibility flag (see
[`[workers]`](/docs/manifest/#workers)). Contexts in the same project don't
need `Fetch` for each other: `consumes` reaches them through Service Bindings.

### The `Idempotency` capability

`Idempotency` makes at-least-once delivery (a retried command, a replayed event) safe:
check `dedup` for a cached outcome, and on a miss, cache the value you compute with
`remember` so a later call with the same key gets it back instead of recomputing:

```bynk,ignore
on reserve(orderId: OrderId) -> ReserveOutcome given Idempotency {
  let cached <- Idempotency.dedup[ReserveOutcome](Json.encode(orderId))
  match cached {
    Some(outcome) => outcome,
    None => {
      let outcome = ... compute the real outcome ...
      let _ <- Idempotency.remember[ReserveOutcome](Json.encode(orderId), outcome, 24.hours)
      outcome
    }
  }
}
```

Both operations take their type argument explicitly (`dedup[ReserveOutcome]`,
`remember[ReserveOutcome]`) — always, even where an argument's type would otherwise make
it obvious, since a capability operation's own type parameter is never inferred (see
[Capabilities & providers](/book/reference/capabilities/#generic-operations)). Calling
`dedup` alone does **not** cache anything — `remember` is what writes the entry, so
forgetting to call it after a cache miss means that key is recomputed every time, not an
error.

The cached type must be **`Codable`**: the value is stored as its JSON encoding, so a
function, an effect or `()` is a compile error at the call site (cache a `Bool` as a presence
marker rather than `()`). A stored value that no longer decodes as the type you ask for, such
as one an older deploy wrote with another shape, reads as a **miss** (`None`), with a warning
on the runtime console, not as a fault: `dedup` is a cache, and the next `remember` replaces
the entry.

On **Cloudflare** `dedup`, `remember` and `claim` are durable and shared between isolates: a
Worker that calls any of them gets a D1 database with no declaration (see
[the idempotency database](/book/guides/projects-build-and-deployment/deploy-to-cloudflare/#the-idempotency-database)),
holding claims and remembered values, and each call costs one round trip to it. On Node and
in the browser they are an in-memory map, lost on restart and not shared between instances.
`dedup` then `remember` is still check-then-act: use `claim` for work that must run once.

A remembered value larger than about 1 MB is not stored on Cloudflare (a warning is logged
and the handler carries on), because `dedup` is a cache. Keys are scoped to the calling
function or handler, so a generic helper that wraps `dedup`/`remember` shares one key space
across every caller of that helper: include whatever tells the callers apart in the key.

#### `claim`: first delivery wins

`dedup` then `remember` is check-then-act: two deliveries of one key that arrive together
can both miss and both run. `claim` is the race-free form. The first call for a key
returns `Claimed`, and every later call within `expiresAfter` returns `AlreadyClaimed`:

```bynk,ignore
consumes bynk { Idempotency, Claim }

on POST("/hooks/:id") (id: String) -> Effect[HttpResult[String]] by Visitor given Idempotency {
  let Claim.Claimed <- Idempotency.claim(id, 7.days) else { Ok("duplicate") }
  Ok("processed")
}
```

`claim` records no outcome, so there is nothing to replay: use it when the work should run
at most once. It is separate from `dedup`/`remember`, which never see a claim.

In a service handler or an agent handler, the compiler settles the claims the handler made when it exits:

- **A failure releases the claim**, so a retry can claim again. A failure is a fault, a
  returned `Err`, or an `HttpResult` that is a 5xx (`ServerError`, `ServiceUnavailable`
  and the rest). A 4xx, or any other result, is a completed outcome, and the claim is held
  for the full `expiresAfter`.
- **A claim is a lease until it is settled.** It holds for at most 30 seconds, matching
  the `waitUntil` limit, and only then for `expiresAfter`. If the instance dies before the
  handler settles, the claim lapses and a retry can claim again. A handler that runs
  longer than 30 seconds can be re-claimed by a concurrent retry while it is still running;
  when it settles it leaves the retry's claim alone, releasing or confirming only a claim
  that is still its own.

The guarantee is **at-most-once completion, not exactly-once side effects**: work done
before a failure, such as a call to another service, can repeat on the retry. Make those
calls idempotent too. A `claim` in a service handler or an agent handler is settled, an agent
handler's after its state commit and events flush, so a failure of either releases it. A
`claim` anywhere else is not settled: it is held for `expiresAfter` whether or not the
work succeeds. Settling an agent's claim releases only the claim: state the handler wrote before
returning an `Err` is still committed, and a retry repeats that write.

The key is scoped to the calling handler as for `dedup`.

`claim` is atomic on Cloudflare: the claim is one `INSERT … ON CONFLICT` in the Worker's D1
database, so two deliveries of one key, on
different isolates or after a restart, yield exactly one `Claimed`. A claim that has
lapsed is taken over, and a few expired rows are deleted with each claim, so the table
stays small. It costs one database round trip per `claim`. On Node and in the browser
`claim` is atomic within one process and lost on restart, as `dedup` is. This is the
Cloudflare platform's binding of the portable `Idempotency` capability, chosen by the
build's platform, not by the author.

#### `claim`: first delivery wins

`dedup` then `remember` is check-then-act: two deliveries of one key that arrive together
can both miss and both run. `claim` is the race-free form. The first call for a key
returns `Claimed`, and every later call within `expiresAfter` returns `AlreadyClaimed`:

```bynk,ignore
consumes bynk { Idempotency, Claim }

on POST("/hooks/:id") (id: String) -> Effect[HttpResult[String]] by Visitor given Idempotency {
  let Claim.Claimed <- Idempotency.claim(id, 7.days) else { Ok("duplicate") }
  Ok("processed")
}
```

`claim` records no outcome, so there is nothing to replay: use it when the work should run
at most once. It is separate from `dedup`/`remember`, which never see a claim.

In a service handler or an agent handler, the compiler settles the claims the handler made when it exits:

- **A failure releases the claim**, so a retry can claim again. A failure is a fault, a
  returned `Err`, or an `HttpResult` that is a 5xx (`ServerError`, `ServiceUnavailable`
  and the rest). A 4xx, or any other result, is a completed outcome, and the claim is held
  for the full `expiresAfter`.
- **A claim is a lease until it is settled.** It holds for at most 30 seconds, matching
  the `waitUntil` limit, and only then for `expiresAfter`. If the instance dies before the
  handler settles, the claim lapses and a retry can claim again. A handler that runs
  longer than 30 seconds can be re-claimed by a concurrent retry while it is still running;
  when it settles it leaves the retry's claim alone, releasing or confirming only a claim
  that is still its own.

The guarantee is **at-most-once completion, not exactly-once side effects**: work done
before a failure, such as a call to another service, can repeat on the retry. Make those
calls idempotent too. A `claim` in a service handler or an agent handler is settled, an agent
handler's after its state commit and events flush, so a failure of either releases it. A
`claim` anywhere else is not settled: it is held for `expiresAfter` whether or not the
work succeeds. Settling an agent's claim releases only the claim: state the handler wrote before
returning an `Err` is still committed, and a retry repeats that write.

The key is scoped to the calling handler as for `dedup`. The in-memory provider makes the
claim atomic within one process; it is no more durable than `dedup`.

The `key` you pass is automatically scoped to the calling handler's own qualified name
before it reaches the provider — `Idempotency.dedup[T]("order-1")` (and `remember`, `claim`) inside `on call` of
service `ordering` in context `shop.reserve` actually stores against
`"shop.reserve.ordering.call::order-1"`. Two unrelated handlers can therefore never
collide on the same literal key by accident; only two calls that are genuinely the *same*
call site (including the same call reached via a cross-context alias) ever share a scope.
This doesn't stop two different *callers* of the same handler from colliding if the key
you supply doesn't itself distinguish them (two tenants both calling `reserve` with the
same order ID, say) — differentiating those is still on you, the same discipline any
caller-supplied idempotency key requires.

### The `Events` capability

`Events` emits an `event` — a typed fact a context declares — for any number
of other contexts to subscribe to with `from Events(E)`:

```bynk
context commerce.order

consumes bynk { Events }
exports transparent { PaymentConfirmed }

event PaymentConfirmed = {
	orderId: String,
}

service markPaid {
	on call(orderId: String) -> Effect[()] given Events {
		Events.emit[PaymentConfirmed](PaymentConfirmed { orderId: orderId })
	}
}
```

`emit`'s type argument is explicit, like `Idempotency`'s operations above —
never inferred from the value's type. Only the context that declares an event
may emit it, even though the type is visible cross-context for subscription
(`bynk.event.emit_outside_owner` otherwise); emission is released only if the
emitting handler itself commits, so an aborted handler emits nothing. See
[Understand events](/book/guides/events/understand-events/) for the full
model, the subscriber side (`from Events(E)`, `on event`), and how delivery
differs between Cloudflare and Bundle targets.

The `bynk` unit also exports the transparent types these operations use:

```bynk
type Uuid       = String where Matches("[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")
type Method     = enum { Get, Post, Put, Delete, Patch, Head, Options }
type FetchError = enum { Network, Timeout, InvalidHeader, InvalidBody }
type Redirect   = enum { Follow, Manual }
type LogLevel   = enum { Debug, Info, Warn, Error }

type FetchOptions = {
  timeout:  Option[Duration] = None,
  redirect: Redirect = Redirect.Follow,
}

type Request = {
  method: Method,
  url: String,
  contentType: Option[String],
  authorization: Option[String],
  headers: Map[String, String],
  body: Option[String],
}

type Response = {
  status: Int,
  headers: Map[String, String],
  body: String,
}

type EventEnvelope = {
  eventId:       String,
  publisherId:   String,
  emittedAt:     Instant,
  schemaVersion: Int,
}
```

`EventEnvelope` is an `on event` handler's optional second parameter — see
[Understand events](/book/guides/events/understand-events/#the-envelope-and-idempotent-handling).

### Request and response headers

`contentType` and `authorization` are typed slots for the two common request
headers. Any other header goes in `headers`; a record field has no default, so a
request that sends none says so with `headers: Map.empty()`:

```bynk,fragment
let req = Request {
  method:        Get,
  url:           "https://api.github.com/repos/bynk-lang/bynk",
  contentType:   None,
  authorization: None,
  headers:       Map.empty()
                   .insert("user-agent", "bynk-examples/1.0")
                   .insert("accept", "application/vnd.github+json"),
  body:          None,
}
```

Header names are case-insensitive. `Fetch.send` returns `Err(InvalidHeader)`
without sending anything when `headers`:

- names `Content-Type` or `Authorization` while the matching typed slot is
  `Some` — neither silently wins. While the slot is `None`, `headers` may
  supply that header itself;
- names a header the platform owns: `host`, `content-length`, `connection`,
  `keep-alive`, `te`, `trailer`, `transfer-encoding`, `upgrade` or `expect`;
- names the same header twice in different case (`X-Id` and `x-id`); or
- holds a name or value that is not a legal HTTP header (a newline in a value,
  say).

A typed slot whose value is not a legal header value (a secret read with a
stray newline) is `Err(InvalidHeader)` too, even with an empty `headers`.
Retrying cannot fix any of these, which is why they are not `Network`.

A `Get` or `Head` request with a `body` is `Err(InvalidBody)`, with nothing
sent: HTTP gives those methods no body. Retrying cannot fix that either.

`Response.headers` carries the response's headers with lowercased keys, so
`res.headers.get("etag")` or `res.headers.get("retry-after")` reads one
whatever case the server sent; a header the server repeated arrives as one
value, joined with `", "`. That includes `set-cookie`, and there the join is
lossy: a cookie's `Expires` attribute contains a comma
(`Expires=Wed, 09 Jun 2027 10:18:14 GMT`), so a joined `set-cookie` cannot be
split back into its cookies reliably. A `stub Fetch.send(_) returns Ok(Response { … })` in a test
gives it `headers: Map.empty()` (or the headers the case needs), and an
`expect Fetch.send called once with req.headers.get("user-agent") == Some("…")`
matches on what was sent.

### Timeouts and redirects

`send` follows redirects and waits as long as the platform does. `sendWith`
takes the same request with a `FetchOptions`:

```bynk,fragment
let res <- Fetch.sendWith(req, FetchOptions { timeout: Some(5.seconds), redirect: Manual })
```

Each field defaults to what `send` does, so a caller writes only the options it
changes: `FetchOptions { timeout: Some(5.seconds) }` bounds the call and still
follows redirects, and `FetchOptions {}` is `send`. The same holds for `KvGet`,
`KvPut`, `R2Put` and `QueueSend` (see [Field defaults](/book/reference/types/#field-defaults)).

- `timeout` bounds the whole exchange, the response body included, except for
  the [streamed operations](#streamed-bodies), where it bounds the response
  head only. When it elapses the result is `Err(Timeout)`. A timeout of zero or less, or over
  2,147,483,647 ms (about 24 days, the longest a platform timer takes),
  faults the call, with nothing sent. `None` waits as long as `send` does.
- `redirect: Follow` follows redirects, as `send` does. `Manual` returns the
  `3xx` itself, with its `location` header, so the service decides whether to
  follow it (to refuse a redirect to an internal host, say).

The options are a record of their own, not fields of `Request`, so `send` keeps
its one-argument form and every request literal stays as it is. A later option
is a new `FetchOptions` field with a default, which no caller has to write. `sendWith` is experimental: list `"bynk.Fetch.sendWith"` under
`[unstable] allow`. A test stubs it as it does `send`:
`stub Fetch.sendWith(_, _) returns Ok(Response { … })`.

`Method` also has `Patch`, `Head` and `Options`. A `Head` response's `body` is
`""`. `Method` is extensible, so a `match` on it needs a wildcard arm.

### Binary bodies

`send` and `sendWith` carry text: a `String` request body, and a response body
decoded as UTF-8, where any byte that is not UTF-8 is lost. Two operations
carry bytes instead, and both return a `BytesResponse`, whose `body` is every
byte the server sent:

```bynk
type BytesResponse = {
  status:  Int,
  headers: Map[String, String],
  body:    Bytes,
}
```

- `receiveBytes(req, options)` sends `req` as `sendWith` does, its `String`
  body included, and returns the response as bytes: a download, or a JSON
  request that answers with a binary.
- `sendBytes(req, body, options)` sends `body` as the request body and returns
  the response as bytes too. `req.body` must be `None`: a request with both is
  `Err(InvalidBody)`, with nothing sent, so neither silently wins. A `Get` or
  `Head` with a `Bytes` body is `Err(InvalidBody)` too. An empty `Bytes` is an
  empty body, not a missing one.

```bynk,fragment
let options = FetchOptions { timeout: Some(10.seconds), redirect: Follow }
let got <- Fetch.receiveBytes(download, options)
let sent <- Fetch.sendBytes(upload, image, options)
```

A binary body gets no `Content-Type` of its own: set `req.contentType` (to
`Some("application/octet-stream")`, say). To read a response that is really
text, call `res.body.decodeUtf8()`, which is `None` when it is not UTF-8.
Bodies are buffered whole, so they suit what fits in a Worker's memory; for
more, see [Streamed bodies](#streamed-bodies). Both operations are experimental: list
`"bynk.Fetch.receiveBytes"` and `"bynk.Fetch.sendBytes"` under
`[unstable] allow`. A test stubs them as it does `send`:
`stub Fetch.sendBytes(_, _, _) returns Ok(BytesResponse { status: 201, headers: Map.empty(), body: Bytes.empty() })`.
In the browser playground they throw, as `send` does.

### Streamed bodies

`receiveStream` and `sendStream` are `receiveBytes` and `sendBytes` without the
buffering. Both return a `StreamResponse` as soon as the response head
arrives, and its body arrives as it is read:

```bynk
type StreamResponse = {
  status:  Int,
  headers: Map[String, String],
  body:    Stream[Bytes],
}
```

- `receiveStream(req, options)` sends `req` as `receiveBytes` does, its
  `String` body included.
- `sendStream(req, body, options)` sends `body` chunk by chunk. `req.body`
  must be `None`, and a `Get` or `Head` can't carry it: both are
  `Err(InvalidBody)`, with nothing sent.

A proxy re-serves an upstream without holding it, through `RawStreamed`:

```bynk,fragment
let got <- Fetch.receiveStream(upstream, FetchOptions { timeout: Some(10.seconds), redirect: Follow })
match got {
  Ok(r) => RawStreamed(r.body, r.headers.get("content-type").getOrElse("application/octet-stream"))
  Err(_) => BadGateway("the upstream could not be reached")
}
```

- **The timeout bounds the head only.** It runs until the response head
  arrives (for `sendStream`, that normally includes sending the body), then
  stops, so a long download isn't cut off. A head that doesn't arrive in time
  is `Err(Timeout)`.
- **A failure partway through the body faults the read.** The result is
  already `Ok`, so a dropped connection surfaces in the `collect`, `map` or
  `RawStreamed` that is reading it; a `RawStreamed` response is cut short.
- **A body is read once.** `r.body` is single-pass, as every `Stream` is: a
  second read faults. A `StreamResponse` carries a stream, so it can't be a
  handler's result, an event field or stored; serve its body with
  `RawStreamed`, or `collect` it.
- **An unread body holds its connection** until the request ends, so a
  proxy shouldn't leave one unread on a busy path. A read that stops early (a
  `take`, say) releases it, as does a `HEAD` answered from a `RawStreamed`
  `GET`.
- **Handing a stream to `sendStream` consumes it,** whatever the outcome,
  `InvalidBody` included. A fault from the stream itself, while it is sent,
  faults `sendStream`; it isn't reported as `Err(Network)`.
- **A streamed upload has no `content-length`.** It is sent chunked, and some
  endpoints refuse that (an S3 presigned `PUT`, for example). Use `sendBytes`
  for those.
- **`sendStream` doesn't follow redirects.** The body can't be sent twice, so a
  `3xx` comes back as it is, `location` included, whatever `options.redirect`
  says. `receiveStream` follows them as `receiveBytes` does.

Both operations are experimental: list `"bynk.Fetch.receiveStream"` and
`"bynk.Fetch.sendStream"` under `[unstable] allow`. A test stubs them as it
does `send`, and a stub's value is built afresh for each call, so each call
gets a stream of its own:
`stub Fetch.receiveStream(_, _) returns Ok(StreamResponse { status: 200, headers: Map.empty(), body: Stream.of([Bytes.fromUtf8("x")]) })`.
In the browser playground they throw, as `send` does.

### Log levels and structured fields

`debug`, `info`, `warn` and `error` write the message as it is, through the
console method of that name (`info` through `console.log`). `log` writes a
structured record:

```bynk,fragment
let _ <- Logger.log(Warn, "slow checkout", Map.empty().insert("orderId", id).insert("ms", ms.toString()))
```

The record is `{ message, level, fields }`, with `level` in lowercase:

- **On Cloudflare** it is logged as one object, and Workers Logs indexes its
  keys, so `fields.orderId` is queryable.
- **Under Node** it is one JSON line.
- **In the browser** it is the message, then the fields.

Fields are nested under `fields`, so a field named `message` or `level` cannot
overwrite the record's own. Values are text: format a number with `toString()`
and a structure with `Json.encode`. `log` is experimental: list
`"bynk.Logger.log"` under `[unstable] allow`. In tests every level is a no-op
that `expect Logger.log called once with …` can observe.

## The Cloudflare surface — `bynk.cloudflare`

`consumes bynk.cloudflare { … }` exposes the platform's own infrastructure.
Consuming it **locks the deployment unit to Cloudflare** (it cannot then target
`node`), and the generated `wrangler.toml` gains the matching binding stanza.

| Capability | Operations |
|---|---|
| **`Kv`** | `get(key: String) -> Effect[Option[String]]` · `put(key: String, value: String) -> Effect[()]` · `putTtl(key: String, value: String, ttlSeconds: Int) -> Effect[()]` · `delete(key: String) -> Effect[()]` · `list(prefix: Option[String]) -> Effect[List[String]]` · *(experimental)* `getWith(key, options: KvGet) -> Effect[Option[KvEntry]]` · `putWith(key, value: String, options: KvPut) -> Effect[()]` · `getBytes(key) -> Effect[Option[Bytes]]` · `putBytes(key, value: Bytes, options: KvPut) -> Effect[()]` · `listPage(options: KvList) -> Effect[KvListing]`; see [Metadata, expiry and binary values](#metadata-expiry-and-binary-values). |
| **`Service`** *(experimental)* | `send(req: Request) -> Effect[Result[Response, FetchError]]`. Used only through a named instance; see below. |
| **`R2`** *(experimental)* | `get(key) -> Effect[Option[Bytes]]` · `head(key) -> Effect[Option[R2Object]]` · `put(key, body: Bytes) -> Effect[R2Object]` · `putWith(key, body, options: R2Put) -> Effect[Option[R2Object]]` · `delete(key) -> Effect[()]` · `list(prefix: Option[String], cursor: Option[String]) -> Effect[R2Listing]` · `getStream(key) -> Effect[Option[R2Body]]` · `putStream(key, body: Stream[Bytes], size: Int) -> Effect[R2Object]` · `putStreamWith(key, body, size, options: R2Put) -> Effect[Option[R2Object]]` · `createUpload(key, options: R2UploadOptions) -> Effect[R2Upload]` · `uploadPart(upload, partNumber: Int, body: Bytes) -> Effect[R2Part]` · `completeUpload(upload, parts: List[R2Part]) -> Effect[R2Object]` · `abortUpload(upload) -> Effect[()]`. Used only through a named instance; see below. |
| **`RateLimit`** *(experimental)* | `allow(key: String) -> Effect[Bool]`. Used only through a named instance that sets its limit and window; see below. |
| **`EdgeCache`** *(experimental)* | `purge(prefixes: List[String]) -> Effect[Bool]`. Used only through a named instance, at most one per context, which turns Cloudflare's edge cache on for the context's Worker; see below. |
| **`Version`** *(experimental)* | `current() -> Effect[WorkerVersion]`. One per Worker, used bare; see [The running version](#the-running-version-version). |
| **`Ai`** *(experimental)* | `run(model: String, input: String) -> Effect[Result[String, AiError]]` · `generate(model, request: GenerateRequest) -> Effect[Result[Generation, AiError]]` · `embed(model, texts: List[String]) -> Effect[Result[List[List[Float]], AiError]]` · `runStream(model, input: String) -> Effect[Result[Stream[String], AiError]]`. One per Worker, used bare; see [Workers AI](#workers-ai-ai). |
| **`StoredSecret`** *(experimental)* | `get() -> Effect[String]`. Used only through a named instance that names its store and its secret; see [Account secrets](#account-secrets-storedsecret). |
| **`Email`** *(experimental)* | `send(message: EmailMessage) -> Effect[Result[String, EmailError]]`. Used only through a named instance, which may restrict its senders and recipients; see [Outbound email](#outbound-email-email). |
| **`Postgres`** *(experimental)* | `all(sql: String, params: List[PgValue]) -> Effect[Result[String, PgError]]` · `first(sql, params) -> Effect[Result[Option[String], PgError]]` · `run(sql, params) -> Effect[Result[PgRun, PgError]]` · `transaction(statements: List[PgStatement]) -> Effect[Result[List[PgRun], PgError]]`, and the compiler-lowered typed reads `select[T]` and `firstAs[T]`. Used only through a named instance; see [Postgres databases](#postgres-databases-postgres). |
| **`MySql`** *(experimental)* | `all(sql: String, params: List[MySqlValue]) -> Effect[Result[String, MySqlError]]` · `first(sql, params) -> Effect[Result[Option[String], MySqlError]]` · `run(sql, params) -> Effect[Result[MySqlRun, MySqlError]]` · `transaction(statements: List[MySqlStatement]) -> Effect[Result[List[MySqlRun], MySqlError]]`, and the compiler-lowered typed reads `select[T]` and `firstAs[T]`. Takes an optional `@tls("…")`. Used only through a named instance; see [MySQL databases](#mysql-databases-mysql). |
| **`Vectorize`** *(experimental)* | `upsert(vectors: List[Vector]) -> Effect[Result[Mutation, VectorError]]` · `insert(vectors) -> Effect[Result[Mutation, VectorError]]` · `query(vector: List[Float], options: QueryOptions) -> Effect[Result[List[VectorMatch], VectorError]]` · `getByIds(ids: List[String]) -> Effect[Result[List[Vector], VectorError]]` · `deleteByIds(ids) -> Effect[Result[Mutation, VectorError]]` · `describe() -> Effect[Result[IndexInfo, VectorError]]`. Used only through a named instance that states its dimensions and metric; see [Vector indexes](#vector-indexes-vectorize). |
| **`AiSearch`** *(experimental)* | `search(query: String, options: SearchOptions) -> Effect[Result[List[SearchHit], SearchError]]` · `answer(messages: List[ChatMessage], options: SearchOptions) -> Effect[Result[SearchAnswer, SearchError]]`. Used only through a named instance that names an existing AI Search instance; see [AI Search](#ai-search-aisearch). |

`Kv` is backed by a Worker KV namespace bound as `env.KV`; the
`[[kv_namespaces]]` stanza is derived for you.

### Several namespaces: derived capabilities

For more than one namespace, declare each as a derived capability:
`capability Sessions = Kv` is a distinct instance of `Kv`, with `Kv`'s
operations and its own namespace, bound as `env.KV_SESSIONS`. A context may use
the bare `Kv` alongside its instances, and the bare `KV` stanza is emitted only
when it does. See [Derived capabilities](/book/reference/capabilities/#derived-capabilities).

### Metadata, expiry and binary values

Five more `Kv` operations reach what the platform has beyond text. They are
experimental: list each one you use under `[unstable] allow`
(`"bynk.cloudflare.Kv.putWith"` and so on). The existing operations are
unchanged.

```bynk
type KvExpiration = | Never | After(ttl: Duration) | At(instant: Instant)
type KvPut      = { expiration: KvExpiration = KvExpiration.Never, metadata: Map[String, String] = Map.empty() }
type KvGet      = { cacheTtl: Option[Duration] = None }
type KvEntry    = { value: String, metadata: Map[String, String] }
type KvKey      = { name: String, expiration: Option[Instant], metadata: Map[String, String] }
type KvList     = { prefix: Option[String], cursor: Option[String], limit: Option[Int] }
type KvListing  = { keys: List[KvKey], cursor: Option[String] }
```

- `putWith(key, value, options)` writes text with an expiry and metadata.
  `After(ttl)` is relative, with a part second rounding up, and `At(instant)` is
  absolute. Either must be at least 60 seconds away. Metadata is at most 1024
  bytes once serialised as JSON. `putTtl` stays as the short form of
  `After`.
- `getWith(key, options)` reads a value with its metadata. `cacheTtl` lets
  Cloudflare's edge serve the read from cache for that long, at least 30
  seconds; `None` keeps the platform's 60.
- `getBytes` and `putBytes` read and write binary values. `putBytes` takes the
  same `KvPut`.
- `listPage(options)` reads one page of keys, in key order, with each key's
  expiry and metadata. `limit` is 1 to 1000 (`None` for 1000). Pass the
  previous page's `cursor` for the next; the last page's is `None`. A page can
  be short and still have a cursor, so loop until the cursor is `None`. The
  draining `list` still returns every key at once.

```bynk,fragment
let _ <- Sessions.putWith(id, body, KvPut { expiration: After(30.minutes), metadata: Map.empty().insert("user", who) })
let page <- Sessions.listPage(KvList { prefix: Some("s:"), cursor: None, limit: Some(100) })
```

Text and binary values share a namespace, as on the platform. `getBytes` of a
text value gives its UTF-8 bytes, and reading a binary value as text decodes it
as UTF-8, replacing what isn't. Keep each key to one kind.

A key over 512 bytes, metadata over the limit, an expiry under 60 seconds away,
a `cacheTtl` under 30 seconds or a `limit` outside 1 to 1000 faults the call
before the platform sees it. A value over 25 MiB faults with the platform's
error. In tests the namespace is in memory: it keeps metadata and expiry
(expiry is not enforced), checks the same limits, and pages in key order.

### Calling a Worker outside the project: `Service`

`Service` calls a Worker that is not part of the Bynk project over a Service
Binding: Cloudflare's private channel, not the public internet that `Fetch`
uses. Each Worker is a named instance that names it, and a bare
`given Service` is refused (`bynk.capability.instance_only`):

```bynk,fragment
consumes bynk
consumes bynk.cloudflare { Service }

capability Billing = Service @worker("legacy-billing")
```

`Billing` binds as `env.SERVICE_BILLING`, a `[[services]]` entry with
`service = "legacy-billing"`. `send` has `Fetch.send`'s shape: the same
`Request`, `Response` and `FetchError`, and the same header rules. The call is
untyped; a typed client is ordinary Bynk over it. `Service` is experimental, so
list `"bynk.cloudflare.Service"` under `[unstable] allow`. See
[Call a Worker outside the project](/book/guides/effects-and-capabilities/call-a-foreign-worker/).

### Object storage: `R2`

`R2` stores objects in an R2 bucket. Each bucket is a named instance, and a
bare `given R2` is refused (`bynk.capability.instance_only`):

```bynk,fragment
consumes bynk.cloudflare
consumes bynk.cloudflare { R2 }

capability Archives = R2
capability Indexes = R2 @jurisdiction("eu")
```

`Archives` binds as `env.R2_ARCHIVES`, an `[[r2_buckets]]` entry. The second
`consumes` line names the types below. `@jurisdiction("eu")` (or `"fedramp"`)
pins the bucket's jurisdiction when `bynk deploy` creates it; any other value
is `bynk.capability.invalid_option`.

`get`, `put` and `putWith` move objects whole, as `Bytes`, buffered in the
Worker's memory, so keep each well under the 128 MB isolate limit. Larger
objects stream, or are uploaded in parts (below).

- `get` and `head` give `None` for a missing key. `head` returns the object's
  `R2Object`: `key`, `size`, `etag`, `uploaded` (an `Instant`), `contentType`
  (`Option[String]`) and custom `metadata` (`Map[String, String]`), without the
  body. `delete` is idempotent.
- `put` writes unconditionally and returns the new `R2Object`.
- `putWith` takes an `R2Put { contentType, metadata, onlyIf }`. `onlyIf` is an
  `R2Condition`: `Always`, `Absent` (create only: what content-addressed
  storage needs) or `EtagMatches(etag)` (compare and swap: what rewriting an
  index needs). It returns `None`, writing nothing, when the condition fails.
  `R2Condition` is extensible, so a `match` over it needs a wildcard.
- `list` returns one `R2Listing { keys, cursor }`: up to 1000 objects in key
  order, optionally under a prefix. Pass the previous page's `cursor` for the
  next; `None` on the last page. A page may be shorter than 1000 while more
  remain, so trust the cursor, not the length.
- `getStream` gives an `R2Body { object, body }`: the object's `R2Object`, and
  its body as a `Stream[Bytes]` read from R2 as it is pulled. Serve it with
  `RawStreamed(o.body, contentType)`, or `collect` it. The body can be read
  once; a second read faults. `R2Body` carries a stream, so it can't be stored,
  sent or JSON-encoded.
- `putStream(key, body, size)` writes a stream of exactly `size` bytes. R2 needs
  a stream's length up front (on Workers it goes through a
  `FixedLengthStream`), so a stream holding more or fewer bytes faults the
  write and nothing is stored. `putStreamWith` adds `putWith`'s `R2Put`. To
  copy an object, pass its `getStream` body with `o.object.size`.
- A multipart upload builds one object from parts, in any order and across
  requests. `createUpload(key, R2UploadOptions { contentType, metadata })`
  returns an `R2Upload { key, uploadId }`, plain data that can be kept or
  handed to a client. `uploadPart(upload, n, bytes)` uploads part `n` (1 to
  10,000) and returns its `R2Part { partNumber, etag }`; `completeUpload`
  takes every part and returns the stored `R2Object`; `abortUpload` discards
  the upload. Every part but the last must be the same size, at least 5 MiB;
  a completion R2 refuses faults, and R2 aborts an upload left incomplete for
  7 days.

A `@raw` handler whose `body` is a `Stream[Bytes]` can pass it to `putStream`, so a
client's upload streams into R2 without being held: read the length from the request's
`content-length` header for `size`. See
[Streamed request bodies](/book/reference/http/#streamed-request-bodies). A chunked
upload has no length, so send a large object that way as a multipart upload, a part a
request.

Each instance has its own in-memory bucket in tests, honouring conditions,
metadata, paging, declared sizes and part rules (all but the 5 MiB minimum, so
a test needn't build 5 MiB of bytes), so `stub Archives.get(_)` and `expect Archives.get called`
work as for any capability. `R2` is experimental: list `"bynk.cloudflare.R2"`
under `[unstable] allow`. `bynk deploy` creates and names the buckets; see
[Deploy to Cloudflare](/book/guides/projects-build-and-deployment/deploy-to-cloudflare/#r2-buckets).

### Rate limiting: `RateLimit`

`RateLimit` counts calls per key over a fixed window, on Cloudflare's Rate
Limiting binding. Each limiter is a named instance that sets both its limit and
its window, and a bare `given RateLimit` is refused
(`bynk.capability.instance_only`):

```bynk,fragment
consumes bynk.cloudflare
consumes bynk.cloudflare { RateLimit }

capability PublishLimit = RateLimit @limit(20) @period(60.seconds)
capability ReadLimit = RateLimit @limit(1000) @period(10.seconds)
```

`PublishLimit` binds as `env.RATELIMIT_PUBLISH_LIMIT`, a `[[ratelimits]]`
entry with `simple = { limit = 20, period = 60 }`.

- `@limit(n)` is the number of calls allowed per key in one window: a positive
  `Int` literal.
- `@period(d)` is the window: `10.seconds` or `60.seconds` (`1.minutes` is the
  same), the only two the platform offers.
- Both are required (`bynk.capability.missing_option`). A limit that isn't a
  positive `Int` literal, or another window, is `bynk.capability.invalid_option`.

`allow(key)` counts one call for `key` and gives `true` while `key` is within
the limit, `false` once it is over. Refuse the request, typically with
`TooManyRequests`:

```bynk,fragment
on PUT("/api/:token") (token: String) -> Effect[HttpResult[String]] by Visitor given PublishLimit {
	let allowed <- PublishLimit.allow(token)
	if allowed {
		Ok("published")
	} else {
		TooManyRequests("slow down: 20 publishes a minute per token")
	}
}
```

**Counting is approximate.** Cloudflare keeps the counts per location, and they
are eventually consistent, so a key can get somewhat more than its limit,
especially from many locations at once. That suits abuse and cost control. For
an exact quota (billing, a hard per-account cap), use an agent, one per key,
as the [rate limiter example](/by-example/projects/rate-limiter/) does. Each
instance counts on its own: two limiters never share counts.

In tests each instance is a counter per key over windows of its `@period`, on
the [harness clock](/book/reference/testing/#time-in-tests), so a case can
exhaust a limiter and recover with `advance Clock by`. `stub PublishLimit.allow(_)`
and `expect PublishLimit.allow called` work as for any capability. `RateLimit`
is experimental: list `"bynk.cloudflare.RateLimit"` under `[unstable] allow`.
`bynk deploy` allocates each limiter's `namespace_id`; see
[Deploy to Cloudflare](/book/guides/projects-build-and-deployment/deploy-to-cloudflare/#rate-limiters).

### Sending to a queue: `Queue`

`Queue` sends messages to a Cloudflare Queue. Each producer is a named
instance that names its queue, and a bare `given Queue` is refused
(`bynk.capability.instance_only`):

```bynk,fragment
consumes bynk
consumes bynk.cloudflare { Queue }

capability OrderEvents = Queue @name("order-events")
```

`OrderEvents` binds as `env.QUEUE_ORDER_EVENTS`, a `[[queues.producers]]`
entry with `queue = "order-events"`.

- `@name("…")` is the queue's name on the Cloudflare account: 1 to 63 lowercase
  letters, digits or `-`, starting and ending with a letter or digit. It is
  required (`bynk.capability.missing_option`); anything else is
  `bynk.capability.invalid_option`. The option is `@name`, not `@queue`,
  because `queue` is a keyword.
- The name is account-wide. Every Worker that names the same queue shares it,
  and a Bynk consumer reads it with `service … from queue("order-events")`.

A body is JSON text, normally `Json.encode(value)`, and the consumer decodes it
with its own message type:

```bynk,fragment
on POST("/orders") (order: Order) -> Effect[HttpResult[String]] by Visitor given OrderEvents {
	let _ <- OrderEvents.send(Json.encode(order))
	Created("queued")
}
```

- `send(body)` sends one message.
- `sendWith(body, QueueSend { delay: Some(30.seconds) })` holds the message
  back before it can be delivered. The delay is whole seconds (a part second
  rounds up), at most 12 hours.
- `sendBatch(bodies)` sends its bodies in one call, all or none.
- `sendValue(message)` takes any `Codable` value and sends exactly the message
  `send(Json.encode(message))` would. The call site does the encoding, so nothing
  can go wrong with the text: `OrderEvents.sendValue(order)`.
- `sendValueWith(message, QueueSend { delay: Some(30.seconds) })` is `sendWith`
  for a `Codable` value, with the same delay rules. `sendValueBatch(messages)`
  is `sendBatch` for a `List` of `Codable` values: all or none, and a message
  that cannot be encoded faults before anything is sent. The message type is
  inferred from the arguments.

Text that is not JSON faults the send, and so does going over the platform's
caps: 128 KB a message, and 100 messages or 256 KB a batch. Delivery is at least
once and the platform stores the messages, so make consumers idempotent and keep
secrets out of bodies. Nothing checks that a producer's message matches the
consumer's message type: `send`'s body is text, encoded by the caller, and
`sendValue` checks only that its argument is `Codable`.

In tests each instance accepts every send, checking that each body is JSON (and
a delay is in range) as the platform binding does; nothing is delivered to a
consumer. `stub OrderEvents.send(_)` and `expect OrderEvents.send called` work as
for any capability. `Queue` is experimental: list `"bynk.cloudflare.Queue"`
under `[unstable] allow`. `bynk deploy` creates the queue when it is missing;
see [Deploy to Cloudflare](/book/guides/projects-build-and-deployment/deploy-to-cloudflare/#queues).

### Writing events: `Analytics`

`Analytics` writes events to a Workers Analytics Engine dataset. Each dataset is
a named instance that takes no options, and a bare `given Analytics` is refused
(`bynk.capability.instance_only`):

```bynk,fragment
consumes bynk
consumes bynk.cloudflare { Analytics }

capability Signups = Analytics
```

`Signups` binds as `env.ANALYTICS_SIGNUPS`, an `[[analytics_engine_datasets]]`
entry. The dataset's name is derived, never written: the context's Worker and
the capability in snake case, so `shop.cart`'s `Signups` writes to
`shop_cart_signups`, and to `shop_cart_signups_staging` under `--env staging`.
Underscores, not hyphens, because the dataset is the table you query in the SQL
API (`SELECT … FROM shop_cart_signups`). An option is
`bynk.capability.unknown_option`, `@dataset("…")` included.

A data point is a `DataPoint`:

```bynk,fragment
on POST("/signup/:plan") (plan: String) -> Effect[HttpResult[String]] by Visitor given Signups {
	let _ <- Signups.write(DataPoint { index: Some(plan), blobs: [plan, "web"], doubles: [1.0] })
	Created("counted")
}
```

- `index` is the sampling key, at most 96 bytes; `None` samples the dataset as
  a whole. In SQL it is `index1`.
- `blobs` are text dimensions, at most 20 and at most 16 KB in total
  (`blob1`…`blob20`, in order).
- `doubles` are numbers, at most 20, each finite (`double1`…`double20`).

A point over a limit faults the write; the platform's own cap of 250 writes per
Worker invocation faults with the platform's error. The write is sent after the
response, so it never delays one. Reading is the account's SQL API, outside
Bynk. The platform keeps data for three months, readable with any account
analytics token, so keep secrets and personal data out of it.

In tests each instance accepts every write within the limits, and faults on one
over them, as the platform binding does. `stub Signups.write(_)` and
`expect Signups.write called` work as for any capability. `Analytics` is
experimental: list `"bynk.cloudflare.Analytics"` under `[unstable] allow`.
`bynk deploy` has nothing to create, since the platform makes a dataset on its
first write; see
[Deploy to Cloudflare](/book/guides/projects-build-and-deployment/deploy-to-cloudflare/#analytics-engine-datasets).

### The running version: `Version`

`Version` reports which version of the Worker is running, from Cloudflare's
Version metadata binding. A Worker has one such binding, so `Version` is used
bare, with no named instances (`capability Build = Version` is
`bynk.capability.base_not_instantiable`):

```bynk,fragment
consumes bynk
consumes bynk.cloudflare { Version, WorkerVersion }

type Health = { version: String, tag: Option[String] }

service api from http {
	on GET("/health") () -> Effect[HttpResult[Health]] by Visitor given Version {
		let v <- Version.current()
		Ok(Health { version: v.id, tag: v.tag })
	}
}
```

`current()` gives a `WorkerVersion`: `id`, the version's id; `tag`, its tag, or
`None` when it was uploaded without one; and `deployed`, the `Instant` the
version was created. It is the same for every call in a deployment. Report it
from a health route, or write it beside an `Analytics` point, to tell which
version served a request.

A Worker whose code reaches `Version`, directly or through a provider's `given`,
declares `[version_metadata]` bound as `env.CF_VERSION_METADATA`; one that does
not declares nothing, and consuming `bynk.cloudflare` for `Version` alone
declares no KV namespace. There is nothing for `bynk deploy` to create.

Under `bynk dev` the id is a local one. In a test, `Version` reports a fixed
version (id `"test"`, no tag, `deployed` at the epoch), so a result never
depends on when or where the test runs, and `stub Version.current() returns …`
gives another. A `system` test's Workers report id `"integration"`. `Version` is
experimental: list `"bynk.cloudflare.Version"` under `[unstable] allow`.

### Workers AI: `Ai`

`Ai` runs a model on Cloudflare's [Workers AI](https://developers.cloudflare.com/workers-ai/)
through the Worker's `[ai]` binding, which needs no API token and no account id.
A Worker has one such binding, so `Ai` is used bare, with no named instances
(`capability Chat = Ai` is `bynk.capability.base_not_instantiable`):

```bynk,fragment
consumes bynk
consumes bynk.cloudflare { Ai, AiError }

service translate from http {
	on POST("/translate") (text: String) -> Effect[HttpResult[String]] by Visitor given Ai {
		let reply <- Ai.run("@cf/meta/m2m100-1.2b", Json.encode(TranslateIn { text: text, source_lang: "en", target_lang: "fr" }))
		match reply {
			Ok(json) => Ok(json),
			Err(ModelNotFound) => NotFound,
			Err(_) => ServerError("model unavailable"),
		}
	}
}
```

`run(model, input)` runs any model by its id. `input` is the model's JSON input
as text and the reply is the model's JSON output as text, so a model Cloudflare
ships tomorrow works today: build the input with `Json.encode` and read the
reply with `Json.decode` in a second step, which returns a matchable error
instead of faulting when a model's output is not the shape you expected. The
model id is a string, checked only for being 1 to 128 characters of
`A-Z a-z 0-9 @ / . _ : -`; an `input` that is not a JSON object is a fault. A
model that answers with binary data (an image, say) is `AiFailed`.

`generate` and `embed` are typed for the two tasks whose shape is the same across
models:

```bynk,fragment
type ChatRole = enum { System, User, Assistant }
type ChatMessage = { role: ChatRole, content: String }
type GenerateRequest = {
	messages: List[ChatMessage],
	maxTokens: Option[Int] = None,
	temperature: Option[Float] = None,
	topP: Option[Float] = None,
	seed: Option[Int] = None,
}
type TokenUsage = { promptTokens: Int, completionTokens: Int }
type Generation = { text: String, usage: Option[TokenUsage] }
```

`generate(model, request)` is chat-style generation: `request.messages` must not be
empty (a fault), a setting left `None` is the model's own default, and the
`Generation` has the model's `text` and, when the platform reported it, its token
`usage`. The text comes from the model's `response`, or from
`choices[0].message.content` for a model that answers in that shape instead. `embed(model, texts)` embeds a batch and returns one vector per text, in
order and all the same length, which pairs with a vector index's `List[Float]` unchanged; an empty `texts`
is a fault. `embed` does not check how many texts one call may carry, since the
limit is the model's and drifts: a call over it is `InvalidInput`. The
embeddings use the platform's default pooling, so an index built from them stays
consistent. Both return `BadReply(message)` when the platform answers but not in
the shape they promise (a model that is not of that kind, or an embedding reply whose vectors are empty or of different lengths), and never fault on
it; `run` returns the model's JSON as given and cannot be a `BadReply`.

`runStream(model, input)` is `run` with the reply streamed, for serving a model's
output as it is produced:

```bynk,fragment
service chat from http {
	on GET("/chat") () -> Effect[HttpResult[String]] by Visitor given Ai {
		let started <- Ai.runStream("@cf/meta/llama-3.2-1b-instruct", "{\"prompt\": \"Tell me a story\"}")
		match started {
			Ok(events) => Streaming(events),
			Err(_) => ServerError("model unavailable"),
		}
	}
}
```

Each element is one event of the model's own stream, as the JSON text of that
event, so the client sees what Workers AI sends (the shape is the model's: a
handler that wants text deltas maps the stream and decodes each event; for a
chat model the events observed on a real account carry a `response` text delta,
which Cloudflare does not document). The
binding asks the model to stream; the terminal `[DONE]` marker is consumed, not
yielded. A refusal before the stream starts (an unknown model, a usage limit) is
the `Err`, so the handler can answer with an ordinary response; a failure after
the first event faults the stream and aborts the response, as a failing producer
does. A model that does not stream is `AiFailed`. A client that hangs up cancels
the model's stream, once the Worker next writes to it. A stream a handler never
reads or serves is never cancelled, as with any `Stream`: read it or serve it. Like every `Stream`, the
result is read once and cannot cross a service boundary: the handler that calls
`runStream` is the one that serves it.

An `AiError` is `ModelNotFound` (an id the platform does not know, or has
retired), `InvalidInput(message)` (a request refused as malformed or too large),
`QuotaExceeded` (a usage limit of the account, or a model its plan does not
include), `Overloaded` (temporary capacity, worth retrying), `BadReply(message)`
(`generate` and `embed` only: the reply was not the shape promised) or
`AiFailed(message)` (anything else, with the platform's message). Cloudflare
adds error codes over time, so a `match` over `AiError` needs a wildcard.

A Worker whose code reaches `Ai`, directly or through a provider's `given`,
declares `[ai]` bound as `env.AI`; one that does not declares nothing. There is
nothing for `bynk deploy` to create.

Workers AI is a remote service on your Cloudflare account, so a call may incur
charges, and a model's reply is untrusted input like any other user text. See
Cloudflare's [Workers AI pricing](https://developers.cloudflare.com/workers-ai/platform/pricing/)
for what applies, and put a `RateLimit` in front of any handler an anonymous
caller can reach. In a test every `Ai` operation faults until it is stubbed
(`stub Ai.run(_, _) returns …`), so a test never reaches Workers AI, and a
`system` test's Workers behave the same. `Ai` is experimental: list
`"bynk.cloudflare.Ai"` under `[unstable] allow`.

### Account secrets: `StoredSecret`

`StoredSecret` reads one secret of a Cloudflare [Secrets Store](https://developers.cloudflare.com/secrets-store/).
A Secrets Store belongs to the account, not to a Worker: its secrets are shared
across Workers, rotated in one place, and bound one secret to a binding. That is
what separates it from [`bynk.Secrets`](#the-portable-surface--bynk), whose per-Worker
secrets are portable and are read by name at run time. Each stored secret is a
named instance that says which store and which secret; both options are
required, and a bare `given StoredSecret` is refused
(`bynk.capability.instance_only`):

```bynk,fragment
consumes bynk
consumes bynk.cloudflare { StoredSecret }

capability StripeKey = StoredSecret @store("payments") @secret("stripe-live")

type Charged = { ok: Bool }

service billing from http {
	on POST("/charge") () -> Effect[HttpResult[Charged]] by Visitor given StripeKey {
		let key <- StripeKey.get()
		Ok(Charged { ok: key.length() > 0 })
	}
}
```

`StripeKey` binds as `env.SECRET_STRIPE_KEY`, a `[[secrets_store_secrets]]`
entry. `@store` and `@secret` are names, each 1 to 255 letters, digits, `-` or
`_` (`bynk.capability.invalid_option`); the store is named, never given by id,
so ids stay out of source.

`get()` returns the value as a `String`, not an `Option`. The binding exists
only if `bynk deploy` found the secret, so a missing value at run time is a
platform fault, not something a handler can act on. Bynk never logs the value;
nor should a handler.

`bynk deploy` **links** a stored secret and never creates one. It resolves the
store by name through Cloudflare's API, checks the secret is in it, and records
the store's id in `bynk.deploy.lock`, keyed by environment, Worker and
binding. A missing store or secret fails the deploy before anything is pushed,
naming both and the command that creates the secret
(`wrangler secrets-store secret create`). It needs `CLOUDFLARE_API_TOKEN` and
`CLOUDFLARE_ACCOUNT_ID`, the token with "Secrets Store: Read"; see
[Deploy to Cloudflare](/book/guides/projects-build-and-deployment/deploy-to-cloudflare/#account-secrets-secrets-store).

Under `bynk dev` the secret is read from the local store, not the account's:
create it with `wrangler secrets-store secret create <store> --name <secret>
--scopes workers` (without `--remote`), where `<store>` is the name written in
`@store`. A test never reads a store: `get()` **faults** until
`stub StripeKey.get() returns "…"` gives it a value, and the value is not
printed by a passing or a failing test. `StoredSecret` is experimental: list
`"bynk.cloudflare.StoredSecret"` under `[unstable] allow`.

### Outbound email: `Email`

`Email` sends mail through Cloudflare's [Email Service](https://developers.cloudflare.com/email-service/)
`send_email` binding. Each use is a named instance, and a bare `given Email` is
refused (`bynk.capability.instance_only`). The instance's annotations are
deploy-time policy, all optional and a closed set (an unknown one is
`bynk.capability.unknown_option`):

| Annotation | Effect on `[[send_email]]` |
|---|---|
| `@to("ops@shop.example")` | `destination_address`: every message goes to this one recipient |
| `@allowedRecipients(["a@x.example", …])` | `allowed_destination_addresses`: the recipients the instance may send to |
| `@allowedSenders(["orders@shop.example", …])` | `allowed_sender_addresses`: the senders the instance may send as |

An address is `name@domain`, with no spaces, control characters, angle brackets,
quotes or commas (`bynk.capability.invalid_option`), and a list has at least one.

```bynk,fragment
consumes bynk
consumes bynk.cloudflare { Email, EmailError, EmailMessage }

capability Mail = Email @allowedSenders(["orders@shop.example"])

service orders from http {
	on POST("/receipt") (to: String) -> Effect[HttpResult[String]] by Visitor given Mail {
		let sent <- Mail.send(EmailMessage {
			from: "orders@shop.example",
			to: [to],
			subject: "Your receipt",
			text: Some("Thanks for your order"),
		})
		match sent {
			Ok(id) => Ok(id)
			Err(RateLimited) => Ok("try again later")
			Err(_) => Ok("not sent")
		}
	}
}
```

`Mail` binds as `env.EMAIL_MAIL`. `send` returns the platform's message id, or an
`EmailError` for an outcome a handler acts on: `SenderNotVerified`,
`RecipientNotAllowed`, `RateLimited`, `DailyLimit`, and `SendFailed(message)` for any
other platform error. Cloudflare has further error codes, so `EmailError` is
extensible and a `match` over it needs a wildcard. The catch-all is `SendFailed`,
not `Failed`, so it never shares a name with `SqlError`'s variant.

`EmailMessage` has `from`, `to`, `subject`, and optionally `cc`, `bcc`,
`replyTo`, `text`, `html`, `attachments` (`EmailAttachment`: `filename`,
`contentType`, `content: Bytes`, `inline`) and `headers`. Addresses are plain
`String`s, `"orders@shop.example"` or `"Shop <orders@shop.example>"`; the
platform judges their syntax. An inline attachment's content id is its filename,
so an `html` body refers to it as `cid:logo.png`.

**Faults, not errors.** A platform refusal is a value, because a handler can act
on it. Breaking a documented limit is a programming error, so the call **faults
before anything is sent**: more than 50 recipients across `to`, `cc` and `bcc`,
more than 32 attachments, more than 25 MiB in total (subject, bodies, headers and
attachments, measured as sent: an attachment counts as base64, a third larger
than its bytes), no recipient at all, neither `text` nor `html`, and custom
`headers` over 16 KB.

**Header injection.** `headers` may not set `From`, `To`, `Cc`, `Bcc`, `Subject`
or `Reply-To` (in any case): the message's own fields set those. A header name
that is empty or holds a colon, a space or a control character, and any name or
value holding a line break (CR, LF, U+0085, U+2028 or U+2029), and a value holding another control character (a tab is fine), is refused too. Each faults before the
call, naming the header but never echoing a value, so data that reaches a header
cannot add another.

**Contents are never logged.** Bynk does not log a message body, a header value
or an attachment's content, and a fault names the limit and the count, never the
content.

`bynk deploy` **checks** each `@allowedSenders` domain and never configures one:
it finds the domain's zone on the account and fails, before anything is pushed
and naming every offender, when the domain has no Email Sending enabled; see
[Deploy to Cloudflare](/book/guides/projects-build-and-deployment/deploy-to-cloudflare/#sender-domains-email-sending).
Without `@allowedSenders`, an unverified `from` surfaces at run time as
`SenderNotVerified`.

Attachments reach the platform as base64 text, not as an `ArrayBuffer`: the
local simulator under `bynk dev` cannot serialise a buffer, and accepts the text,
so an attachment works locally and `--remote` is not needed for one.

A handler that sends mail is a spam relay if anyone can call it: put a
[`RateLimit`](#rate-limiting-ratelimit) in front of it.

A test needs no stub: the unit double is the binding's own provider over an
in-memory outbox, so a send is recorded for `expect Mail.send called with …`,
answers a deterministic id (`"test-message-1"`, then `-2`), and faults on the same
limits and headers. `stub Mail.send(_) returns Err(SenderNotVerified)` forces an
error. At `integration` and `system` each Worker binds its own outbox. `Email` is
experimental: list `"bynk.cloudflare.Email"` under `[unstable] allow`.

### Postgres databases: `Postgres`

`Postgres` queries an existing Postgres database through
[Cloudflare Hyperdrive](https://developers.cloudflare.com/hyperdrive/), over
[postgres.js](https://github.com/porsager/postgres). Each database is a named
instance that takes no options, and a bare `given Postgres` is refused
(`bynk.capability.instance_only`). It is **experimental**: list
`"bynk.cloudflare.Postgres"` under `[unstable] allow`. The step-by-step version
is [Connect an existing Postgres database](/book/guides/effects-and-capabilities/connect-an-existing-postgres-database/).

```bynk,fragment
consumes bynk
consumes bynk.cloudflare { Postgres, PgError, PgValue }

capability Main = Postgres

service orders from http {
	on GET("/orders/:id") (id: Int) -> Effect[HttpResult[String]] by Visitor given Main {
		let row <- Main.first("select id, name from orders where id = $1", [PgInteger(id)])
		match row {
			Ok(Some(json)) => Ok(json)
			Ok(None) => Ok("not found")
			Err(PgConstraint(_, _)) => Ok("conflict")
			Err(_) => Ok("failed")
		}
	}
}
```

`Main` binds as `env.HYPERDRIVE_MAIN`, a `[[hyperdrive]]` entry whose `id`
`bynk deploy` fills in. `bynk.toml` says where the connection string comes from,
by the name of an environment variable and never the string:
`[deploy.hyperdrive.Main] connection_env = "MAIN_DATABASE_URL"` makes (or updates)
the Hyperdrive config at deploy, or `config_id = { production = "…" }` adopts one
that exists; `[dev.hyperdrive.Main] connection_env = "MAIN_LOCAL_DATABASE_URL"`
names the local database `bynk dev` serves. See
[Deploy to Cloudflare](/book/guides/projects-build-and-deployment/deploy-to-cloudflare/#postgres-databases-hyperdrive)
and [Connect an existing Postgres database](/book/guides/effects-and-capabilities/connect-an-existing-postgres-database/).

**The statement.** The SQL is a string literal (or a `String` constant bound to
one), as for [`D1`](#sql-databases-d1), so data reaches a statement only as a
parameter: placeholders are `$1`, `$2`, …, and `params` binds them in order.
Bynk does not check the statement or the parameter count: Postgres does, and
answers a `PgError`.

| `PgValue` | Binds |
|---|---|
| `PgNull` | `NULL` |
| `PgInteger(Int)` · `PgReal(Float)` | a number |
| `PgText(String)` | text |
| `PgBytea(Bytes)` | `bytea` |
| `PgBoolean(Bool)` | `boolean` |
| `PgTimestamp(Instant)` | a `timestamptz` |
| `PgJson(String)` · `PgNumeric(String)` | JSON text and a decimal as text, which the server types from the column or a cast (`$1::jsonb`, `$2::numeric`) |

The variants carry the `Pg` prefix because a bare `Null`, `Integer`, `Real`,
`Text` or `Boolean` would resolve to `SqlValue`'s. `PgError` is likewise
`PgConstraint`, `PgSerialization`, `PgConnection` and `PgFailed`, since
`Constraint` and `Failed` are `SqlError`'s.

**The rows.** `all` gives every row as a JSON array of objects keyed by column
name, `first` the first row as one object or `None`, for `Json.decode`. The rules
are fixed: `int8` and `numeric` arrive as **strings** (so nothing past 2^53 is
lost), `timestamptz` as ISO-8601 text, `bytea` as base64 (a `Bytes` field decodes
it), `json` and `jsonb` nested as JSON, and a `boolean` as `true` or `false`
(`D1`'s `0` and `1` do not apply). `first` takes the first row of the statement
as written; add `limit 1` when you want the database to stop early. `run` reports
`PgRun { rowCount }`: the rows the statement inserted, updated or deleted, or a
`SELECT` returned.

**The errors.** Every operation answers a `PgError` rather than faulting:

| Variant | When |
|---|---|
| `PgConstraint(sqlstate, message)` | an integrity-constraint violation, SQLSTATE class `23`: unique, not-null, check, foreign key |
| `PgSerialization(message)` | a serialization failure, `40001`, which a retry may clear |
| `PgConnection(message)` | the connection could not be made or was lost; the message is the driver's code, never the host |
| `PgFailed(sqlstate, message)` | anything else, with its SQLSTATE (empty when the driver gave none) |

Postgres has further failure kinds, so `PgError` is extensible and a `match`
over it needs a wildcard. A missing binding is a deployment mistake and faults.

**Connections.** Each statement opens its own connection from
`env.HYPERDRIVE_MAIN.connectionString`, runs with `{ prepare: true }` (so
Hyperdrive can cache it), and closes it before it answers; a Worker may not carry
a socket between requests, and Hyperdrive pools the connections to your database.
Cloudflare allows **six simultaneous connections per Worker invocation**, so a
[`par`](/book/guides/effects-and-capabilities/run-independent-calls-at-the-same-time/)
over more than six statements waits for its turn. The connection string is a
secret: it is never logged and never appears in an error.

**A Worker with a `Postgres` instance** gets `postgres` (pinned to an exact
version) in `out/package.json`, and a `first-party` `[[npm]]` row in `bynk.lock`
that needs no approval. It needs `nodejs_compat`, which the compatibility date
the toolchain emits already provides. `bynk dev` and `bynk deploy` stop and name
the package when it is not installed in the build directory.

**Tests.** There is no in-memory Postgres: a test never reaches a database, so
every operation of an instance **faults until the case stubs it**, as a
`Service` does: `stub Main.run(_, _) returns Ok(PgRun { rowCount: 1 })`, or
`returns Err(PgConstraint("23505", "duplicate key"))` to force an error. At
`integration`, each Worker's `env.HYPERDRIVE_MAIN` faults on its connection
string until the case stubs the instance. `Postgres` has no `system`-tier
behaviour, where a stub has no effect.

A typed read lowers over `all` and `first`, so stubbing those stubs it, with the
rows as JSON text: `stub Main.all(_, _) returns Ok("[{\"id\":\"1\"}]")`.
`transaction` faults the same way until `stub Main.transaction(_) returns …`.

#### Postgres typed reads: `select[T]` and `firstAs[T]`

`select[T]` and `firstAs[T]` read rows straight into a record, as they do for
[`D1`](#d1-typed-reads), so the column list is not spelt twice.
The first argument is the statement **from `FROM` on**, a string literal (or a
`String` constant bound to one) that must not begin with `SELECT`
(`bynk.types.select_tail_has_select`); the compiler writes `SELECT` and `T`'s
fields as quoted identifiers, in declaration order, ahead of it at compile time,
so the SQL is still a constant. Quoting is Postgres': double quotes, with an
embedded quote doubled.

```bynk,fragment
type Order = { id: Int, name: String, placed: Instant, note: Option[String] }

service feed {
	on call(min: Int) -> Effect[Result[List[Order], PgReadError]] given Main {
		Main.select[Order]("FROM orders WHERE id > $1 ORDER BY id", [PgInteger(min)])
	}
}
// runs: SELECT "id", "name", "placed", "note" FROM orders WHERE id > $1 ORDER BY id
```

A field selects the column of the same name; there is no renaming, so a computed
column stays with `all` and `Json.decode`. The tail is a literal
(`bynk.types.sql_not_literal`), and `T` is the row itself, so `select[List[Row]]`
is refused (`bynk.types.sql_row_shape`).

`select` gives `Result[List[T], PgReadError]` and `firstAs`
`Result[Option[T], PgReadError]`. `PgReadError` is `PgSql(PgError)` for the
statement's own failure and `PgDecode(DecodeError)` for a row that did not
decode. The variants carry the `Pg` prefix because `Sql` and `Decode` are
`SqlReadError`'s.

A row field is an `Int`, `Float`, `String`, `Bool`, `Bytes`, `Decimal` or
`Instant` (or a refined or opaque type over one, or an `Option` of either). Two
of the rows' text forms are read for you, and only in a typed read:

- An `Int` field reads an `int8` that arrives as text, when it is a whole number
  within ±2^53. A larger one fails the decode, naming the field, rather than
  losing digits; read it as a `Decimal` or a `String` column instead.
- An `Instant` field reads a `timestamptz`'s ISO-8601 text and normalises it to
  UTC. The forms are the driver's (`2026-11-11T12:34:56.789Z`) and Postgres' own
  text (`2026-11-11 12:34:56.789+00`, `…+05:30`, `…-0800`, a fraction of up to six
  digits, truncated to milliseconds). A `timestamp` column (no zone, so no
  instant), a `date` column, `infinity` and a `BC` year fail the decode, and a
  `String` field reads each as the server's text: the executor keeps those as
  text, and only a finite `timestamptz` is read as an instant. `Json.decode`
  still reads an `Instant` as epoch milliseconds, as before.

#### Postgres transactions: `transaction`

`transaction(statements)` runs a fixed list of statements **in order, all or
none**, on one connection. A `PgStatement` is `{ sql, params }`, with the `sql` a
literal as for `all`; if one statement fails, every earlier one is rolled back,
the later ones do not run, and the result is that statement's `PgError`. It gives
one `PgRun` per statement, in order.

```bynk,fragment
Main.transaction([
	PgStatement { sql: "insert into orders (name) values ($1)", params: [PgText(name)] },
	PgStatement { sql: "update stock set qty = qty - 1 where name = $1", params: [PgText(name)] },
])
```

A `PgStatement` stays where it is built, as a `SqlStatement` does: it cannot be
stored, sent, received or decoded (`bynk.types.sql_statement_at_boundary`), since
one from outside would carry SQL that was never a literal. An empty list is
`Ok([])` and opens no connection (a missing binding still faults first).

`transaction` runs a list known up front. A read whose result decides a later
write needs the transaction held open between them; Bynk does not offer that, so
make the decision in one statement (`insert … select`, `update … where`) or retry
on `PgSerialization`.

#### Postgres SQL checked for syntax {#postgres-sql-checked-for-syntax}

Bynk does not own your Postgres schema: it is made and changed outside the
project, so there are no migrations to check a statement against, as there are for
[`D1`](#sql-checked-against-the-migrations). A project that keeps a schema file
can still have each literal statement checked. Put a `schema.sql` at the project
root (beside `bynk.toml`) and `bynk check` checks every literal statement of a
`Postgres` capability, in `all`, `first`, `run` and the statements of a
`transaction`:

- `bynk.postgres.sql_param_count` (an **error**): the statement's highest `$n`
  differs from the number of elements in the `params` list written out in place
  (or `let`-bound). The count follows Postgres' rule and is exact: a `$n` inside a
  string, a quoted name, a comment or a dollar-quoted body is text, not a
  parameter.
- `bynk.postgres.sql_invalid` (a **warning**): the statement does not parse with
  the Postgres dialect of [sqlparser](https://github.com/apache/datafusion-sqlparser-rs),
  or is more than one command (a prepared statement takes one). It is a warning
  because that parser is not Postgres' own and can refuse valid SQL, a newer or
  rarer form; it never blocks a build.

Nothing is checked against the tables and columns the file defines: the file's
presence is the switch, and the same file seeds the
[PGlite test double](/book/reference/testing/#postgres-under-test). Without a
`schema.sql` nothing is checked, and deleting it turns the check off. The typed
reads (`select[T]`, `firstAs[T]`) are not checked, since their text is a tail the
compiler completes. The parser is pure Rust, so the check runs everywhere Bynk
does, the playground included.

### MySQL databases: `MySql`

`MySql` queries an existing MySQL (or MariaDB) database through
[Cloudflare Hyperdrive](https://developers.cloudflare.com/hyperdrive/), over
[mysql2](https://github.com/sidorares/node-mysql2). It is `Postgres`'s twin: the
same operations, the same row and transaction rules and the same deploy path, with
MySQL's placeholders, types and errors. Each database is a named instance, and a
bare `given MySql` is refused (`bynk.capability.instance_only`). It is
**experimental**: list `"bynk.cloudflare.MySql"` under `[unstable] allow`. The
step-by-step version is
[Connect an existing MySQL database](/book/guides/effects-and-capabilities/connect-an-existing-mysql-database/).

```bynk,fragment
consumes bynk
consumes bynk.cloudflare { MySql, MySqlError, MySqlValue }

capability Legacy = MySql

service orders from http {
	on GET("/orders/:id") (id: Int) -> Effect[HttpResult[String]] by Visitor given Legacy {
		let row <- Legacy.first("select id, name from orders where id = ?", [MySqlInteger(id)])
		match row {
			Ok(Some(json)) => Ok(json)
			Ok(None) => Ok("not found")
			Err(MySqlConstraint(_, _)) => Ok("conflict")
			Err(_) => Ok("failed")
		}
	}
}
```

`Legacy` binds as `env.HYPERDRIVE_LEGACY`, a `[[hyperdrive]]` entry (the prefix is
`Postgres`'s, since both are Hyperdrive). `bynk.toml` names the connection string's
environment variable exactly as for `Postgres`
(`[deploy.hyperdrive.Legacy] connection_env`), and the string is a
`mysql://user:password@host:3306/database` URL: it is a secret, and never reaches
the ledger, a generated file, a log, the plan or an error message.

**TLS.** `@tls("…")` says how Hyperdrive secures its connection **to your
database**; the Worker's own connection to Hyperdrive is not part of it. The modes
are `"required"` (the default: encrypted, certificate not checked), `"verify-ca"`
and `"verify-identity"` (the server's certificate is checked against a CA
certificate, and for `"verify-identity"` its host name too). Anything else is a
compile error (`bynk.capability.invalid_option`). `bynk build` writes the mode
beside the Worker's `wrangler.toml`, in `bynk-hyperdrive.json`, which also tells
`bynk deploy` the config is MySQL's. `bynk deploy` sends `"required"` as the
config's `sslmode` (`REQUIRED`). The two `verify` modes need a CA certificate your
Cloudflare account holds, which `bynk deploy` does not upload: for a `verify` mode
it stops and says to upload the CA (`wrangler cert upload certificate-authority`),
make the config with `wrangler hyperdrive create … --ca-certificate-id <id>
--sslmode VERIFY_IDENTITY`, and adopt it with `config_id`. Cloudflare does not
support `DISABLED` or `PREFERRED`.

**The statement.** The SQL is a string literal (or a `String` constant bound to
one), so data reaches a statement only as a parameter. Placeholders are `?`, and
`params` binds them in order. mysql2's `query` substitutes each `?` that is not inside a
single-quoted string, a backtick name or a `-- ` or `/* */` comment with an escaped value
before the statement is sent. **A `?` inside a `"…"` string, or after a `#`, is
substituted too, so it counts as a parameter:** write string literals with single
quotes, and comments with `--`. Bynk uses `query`, not a prepared statement, because a prepared
statement does not survive Hyperdrive's transaction pooling and cannot bind
`LIMIT ?, ?`. `??` (mysql2's identifier placeholder) is not supported.

| `MySqlValue` | Binds |
|---|---|
| `MySqlNull` | `NULL` |
| `MySqlInteger(Int)` · `MySqlReal(Float)` | a signed number |
| `MySqlUnsigned(String)` | an unsigned integer, as its digits: an `Int` is signed, and a `BIGINT UNSIGNED` reaches past it |
| `MySqlText(String)` | text |
| `MySqlBytes(Bytes)` | a `BLOB` |
| `MySqlBoolean(Bool)` | `1` or `0`: a `TINYINT(1)`, MySQL's flag |
| `MySqlDecimal(String)` · `MySqlJson(String)` | a `DECIMAL` as text and JSON text for a `JSON` column, so neither loses a digit |
| `MySqlDateTime(String)` | the wall-clock text of a `DATETIME` (`2026-11-12 09:30:00`): no zone, not an instant |
| `MySqlTimestamp(Instant)` | an `Instant` as UTC text (`2026-11-12 09:30:00.000`): right for a `DATETIME` that holds UTC by convention, and for a `TIMESTAMP` only when the session's zone is UTC |

The variants carry the `MySql` prefix so nothing collides with `PgValue`'s or
`SqlValue`'s.

**The rows.** `all` gives every row as a JSON array of objects keyed by column
name, `first` the first row as one object or `None`, for `Json.decode`. The rules
are fixed: a `BIGINT` (signed or unsigned) and a `DECIMAL` arrive as **strings**,
a `TINYINT(1)` as `true` or `false` (any other `TINYINT` is a number, and `D1`'s
`0` and `1` do not apply), a `BLOB` as base64, a `JSON` column nested, and
`DATETIME`, `TIMESTAMP` and `DATE` as the server's text with no zone put on it
(Bynk does not invent an instant: an `Instant` field refuses it and a `String`
field reads it). `run` reports `MySqlRun { rowCount, insertId }`: the rows affected
and the `AUTO_INCREMENT` id the statement inserted (`0` when none),
since MySQL has no `RETURNING`. An id or a count past 2^53 is not an `Int`, so
`run` answers `Err(MySqlFailed(0, …))` naming it instead of a `0` that would read
as "inserted none" (outside a transaction the statement has still been applied, so do not retry on it blindly; inside one, the failure rolls the transaction back).

**The errors.** Every operation answers a `MySqlError` rather than faulting, keyed
on MySQL's error number:

| Variant | When |
|---|---|
| `MySqlConstraint(code, message)` | `1062` duplicate key, `1048` null in a `NOT NULL` column, `1452` and `1451` a foreign key, `3819` a check |
| `MySqlSerialization(code, message)` | `1213` a deadlock or `1205` a lock wait timeout, which a retry may clear |
| `MySqlConnection(message)` | the connection could not be made or was lost; the message is the driver's code, never the host, the user or the password |
| `MySqlFailed(code, message)` | anything else, with its error number (`0` when the driver gave none) |

MySQL has further failure kinds, so `MySqlError` is extensible and a `match` over
it needs a wildcard.

**Connections.** Each statement opens its own connection from
`env.HYPERDRIVE_LEGACY`'s `host`, `port`, `user`, `password` and `database`, with
`disableEval: true` (Workers forbid `eval`, which mysql2 otherwise uses), and
closes it before it answers. Cloudflare's guide asks for mysql2 **3.13.0 or
later** and `nodejs_compat`, which the compatibility date the toolchain emits
already provides. A Worker with a `MySql` instance gets `mysql2` (pinned to an
exact version) in `out/package.json`, and a `first-party` `[[npm]]` row in
`bynk.lock` that needs no approval. Cloudflare allows six simultaneous connections
per Worker invocation, as for `Postgres`.

**Typed reads and transactions.** `select[T]` and `firstAs[T]` and `transaction`
behave as `Postgres`'s, with backtick-quoted columns (an embedded backtick
doubled), a `MySqlStatement { sql, params }` per transaction statement, and
`MySqlReadError` (`MySqlSql`, `MySqlDecode`). A row field is an `Int`, `Float`,
`String`, `Bool`, `Bytes` or `Decimal` (or a refined or opaque type over one, or an
`Option` of either); an `Int` reads a `BIGINT` that arrives as text.

**Tests.** There is no in-memory MySQL: a test never reaches a database, so every
operation of an instance **faults until the case stubs it**:
`stub Legacy.run(_, _) returns Ok(MySqlRun { rowCount: 1, insertId: 41 })`, or
`returns Err(MySqlConstraint(1062, "Duplicate entry"))` to force an error.
`expect Legacy.run called with sql == "…"` works as for any capability. A
`MySql` instance has no PGlite-style double and no `system`-tier behaviour.

#### MySQL SQL checked for syntax {#mysql-sql-checked-for-syntax}

A project that keeps a `schema.sql` at its root (beside `bynk.toml`) has each
literal `MySql` statement checked, in `all`, `first`, `run` and the statements of a
`transaction`, with [sqlparser](https://github.com/apache/datafusion-sqlparser-rs)'s
MySQL dialect. It is the switch `Postgres`'s check uses:

- `bynk.mysql.sql_param_count` (an **error**): the statement's `?` placeholders
  differ in number from the elements of the `params` list written out in place (or
  `let`-bound). A `?` inside a single-quoted string, a backtick name or a comment
  (`-- ` or `/* */`) is text, not a parameter. The count follows mysql2, which
  substitutes inside a double-quoted string and after `#` too: write string
  literals with single quotes.
- `bynk.mysql.sql_invalid` (a **warning**): the statement does not parse, or is more
  than one command. It is a warning because the parser is not MySQL's own (and
  MariaDB has forms of its own); it never blocks a build.

Nothing is checked against the tables the file defines. Without a `schema.sql`
nothing is checked, and the typed reads are not checked either.

### Vector indexes: `Vectorize`

`Vectorize` is a Cloudflare [Vectorize](https://developers.cloudflare.com/vectorize/)
index: vectors with an id, searched by similarity. It is used only through a named
instance that states the index's fixed shape, because Cloudflare cannot change an
index's dimensions or metric after it exists:

```bynk,fragment
consumes bynk
consumes bynk.cloudflare { Vectorize, Vector, QueryOptions, Condition, FilterEq, MetaText }

capability Docs = Vectorize @dimensions(768) @metric("cosine") @indexed(["category:string", "year:number"])

service search from http {
	on GET("/search") (q: String) -> Effect[HttpResult[String]] by Visitor given Docs {
		let hits <- Docs.query(embedding, QueryOptions {
			topK: 5,
			filter: [Condition { field: "category", op: FilterEq(MetaText("faq")) }],
		})
		match hits {
			Ok(found) => Ok(Json.encode(found)),
			Err(_) => ServerError("search unavailable"),
		}
	}
}
```

`capability Docs = Vectorize …` binds as `VECTORIZE_DOCS`, a `[[vectorize]]` entry
naming the index. `@dimensions(n)` (an `Int` from 32 to 1536, the range Cloudflare creates an index with; one outside it is `bynk.capability.invalid_option`) and `@metric("cosine" |
"euclidean" | "dot-product")` are both required; a bare `given Vectorize` is
`bynk.capability.instance_only`. `@indexed([…])` lists the metadata properties a
query may filter on, each written `name:type` with the type `string`, `number` or
`boolean`, at most 10; a malformed entry, an unknown type or more than 10 is
`bynk.capability.invalid_option`. Nothing ties `@dimensions` to an embedding model:
`Ai.embed`'s output length is a runtime fact (768 for
`@cf/baai/bge-base-en-v1.5`, which pairs with `@metric("cosine")`), and a vector of
the wrong length is the platform's `DimensionMismatch`, returned as a value.

A `Vector` is `{ id, values: List[Float], metadata: Option[String], namespace:
Option[String] }`. `upsert` replaces a vector with the same id in full (the platform
does not merge values or metadata) and `insert` keeps an existing id's vector and
writes only new ones. Metadata is a JSON object as text: write it with `Json.encode`
and read it back with `Json.decode` in a second step, so a vector written by an older
version of the program can be skipped or repaired instead of faulting the query.
`query(vector, options)` returns the nearest matches, best first, as `VectorMatch`es
with a `score` whose meaning depends on the metric (a larger cosine score is nearer;
a smaller euclidean distance is nearer). `QueryOptions` carries `topK` (5 by default),
`returnValues`, `returnMetadata` (`Omit`, `Indexed` or `All`), an optional `namespace`
and `filter`, a list of `Condition`s that must all hold. A condition tests one
metadata property with `FilterEq`, `FilterNe`, `FilterLt`, `FilterLte`, `FilterGt`,
`FilterGte`, `FilterAnyOf` or `FilterNoneOf`, over a `MetaText`, `MetaNumber` or
`MetaFlag`. The filter is typed, so a user-supplied string can never become an
operator. Before calling, the binding refuses what the platform cannot take whatever
the data (an empty property name, one with a double quote or a leading `$`, one over
512 characters, a range over a flag, or conditions on one property that are not one
lower and one upper bound) as `VectorInvalidInput`; the limits that drift (the
filter's size, `topK`, a batch, metadata) are the platform's to say, and come back as
`VectorInvalidInput(message)` as well. A property can be filtered on only when it is
in `@indexed` and the vector was written after the metadata index existed.

A write (`upsert`, `insert`, `deleteByIds`) returns a `Mutation { id }` and is
queryable only once the platform has processed it, which takes some seconds: a
handler that writes and then queries in the same request will not see its own write.
`getByIds` reads vectors back with their values and metadata, and `describe` reports
the index's `dimensions` and `metric`.

A `VectorError` is `DimensionMismatch(message)`, `VectorInvalidInput(message)`,
`VectorRateLimited` (worth retrying later) or `VectorFailed(message)` (anything else,
with the platform's message), the `Vector` prefix because `InvalidInput`,
`RateLimited` and `Failed` belong to other error types. Cloudflare adds error cases
over time, so a `match` over `VectorError` needs a wildcard. A call on an index that
does not exist is a platform fault: the deploy creates the index.

A Worker whose code reaches a `Vectorize` instance declares `[[vectorize]]` with
`binding` and `index_name`, the index named for the context and the capability
(`shop.cart`'s `Docs` is `shop-cart-docs`). (`shop.cart`'s `Docs` is `shop-cart-docs`). In a test each `Vectorize` instance is
an exact in-memory index built from its declaration, so a test never reaches
Cloudflare and a retrieval flow runs end to end. It ranks exactly where production
ranks approximately, and a write is visible at once where production takes some
seconds; those are its two differences. It enforces the dimensions, namespaces,
insert versus upsert, all eight filter operators and the platform's limits as read
on 13 November 2026 (a limit it refuses is `VectorInvalidInput`); a filter on a
property that is not in `@indexed` faults, naming the difference, since production
silently matches nothing for it. `stub Docs.query(_, _) returns …` still replaces
any operation. Vectorize is a remote service on your Cloudflare account, so a call
may incur charges; see Cloudflare's [Vectorize pricing](https://developers.cloudflare.com/vectorize/platform/pricing/)
for what applies, and put a `RateLimit` in front of any handler an anonymous caller
can reach. `Vectorize` is experimental: list `"bynk.cloudflare.Vectorize"` under
`[unstable] allow`.

### AI Search: `AiSearch`

`AiSearch` is a Cloudflare [AI Search](https://developers.cloudflare.com/ai-search/)
instance: a managed retrieval service that indexes a data source (an R2 bucket, a
website, files uploaded to it) and answers from it. Unlike a `Vectorize` index, an
instance is configured and owned by someone (its data source, models, chunking and
sync), so Bynk never creates it: you make it in the Cloudflare dashboard, with
`wrangler` or the REST API, and the Worker only queries it. It is used only through a
named instance that names it:

```bynk,fragment
consumes bynk
consumes bynk.cloudflare { AiSearch, ChatMessage, User, SearchOptions }

capability Kb = AiSearch @instance("support-docs")

service ask from http {
	on POST("/ask") (body: String) -> Effect[HttpResult[String]] by Visitor given Kb {
		let reply <- Kb.answer([ChatMessage { role: ChatRole.User, content: body }], SearchOptions {})
		match reply {
			Ok(a) => Ok(a.text),
			Err(_) => ServerError("search unavailable"),
		}
	}
}
```

`capability Kb = AiSearch @instance("support-docs")` binds as `AI_SEARCH_KB`, an
`[[ai_search]]` entry naming the instance as written, in every environment.
`@instance("…")` is required; a bare `given AiSearch` is
`bynk.capability.instance_only`, and a missing or empty name is
`bynk.capability.missing_option` or `bynk.capability.invalid_option`.

`search(query, options)` returns the best-matching chunks, best first, as `SearchHit`s
(`id`, `score` from 0 to 1, `text`, `key` the source path or URL, `timestamp` in unix milliseconds and
`metadata` as JSON text when the item has them), for a handler that shows results or
builds its own prompt with `Ai.generate`. `answer(messages, options)` retrieves the
same way and has the instance's own generation model write the reply from the
conversation, which must not be empty; a `SearchAnswer` is `{ text, hits, usage }`. The
generation model is the instance's setting and a call cannot override it. Retrieved
text and generated answers are untrusted input.

`SearchOptions` carries `maxResults` (1 to 50), `matchThreshold` (0 to 1) and
`retrieval` (`Vector`, `Keyword` or `Hybrid`), each `None` to leave the platform's
default; `rewriteQuery` and `rerank`; and `filter`, a list of the same typed
`Condition`s `Vectorize` takes, which must all hold, over the items' `folder`,
`timestamp` and `filename` and the instance's custom metadata. A text value is
matched on its first 64 bytes only. The binding refuses an out-of-range option, or a
filter the platform cannot take (an empty attribute name, a leading `$`, a range over
a flag, or conditions on one attribute that are not one lower and one upper bound), as
`SearchInvalidInput`.

A `SearchError` is `SearchInvalidInput(message)`, `SearchRateLimited` (worth retrying
later), `SearchQuotaExceeded` or `SearchFailed(message)` (anything else, and a reply
that is not the shape the operation promises), the `Search` prefix because the other
names belong to other error types. Cloudflare adds error cases over time, so a
`match` over `SearchError` needs a wildcard. A call on an instance that does not
exist is a platform fault. A freshly written file is not searchable until the instance
has indexed it, and a query against an instance that has indexed nothing is an empty
list, not an error.

In a test every `AiSearch` operation faults until it is stubbed
(`stub Kb.search(_, _) returns …`), so a test never reaches AI Search; `Vectorize` has
an in-memory double for retrieval logic a test wants to run. AI Search is a remote
service on your Cloudflare account, so a call may incur charges; see Cloudflare's
[AI Search pricing](https://developers.cloudflare.com/ai-search/platform/limits-pricing/)
for what applies, and put a `RateLimit` in front of any handler an anonymous caller
can reach. `AiSearch` is experimental: list `"bynk.cloudflare.AiSearch"` under
`[unstable] allow`.

### SQL databases: `D1`

`D1` queries a D1 database, Cloudflare's SQLite. Each database is a named
instance that takes no options, and a bare `given D1` is refused
(`bynk.capability.instance_only`):

```bynk,fragment
consumes bynk
consumes bynk.cloudflare { D1 }

capability Orders = D1
```

`Orders` binds as `env.D1_ORDERS`, a `[[d1_databases]]` entry. The database's
name is derived, never written: the context's Worker and the capability in
kebab case, so `shop.cart`'s `Orders` is `shop-cart-orders`, and
`shop-cart-orders-staging` under `--env staging`. An option is
`bynk.capability.unknown_option`.

Statements are SQL text with positional parameters, `?1`, `?2` and so on. Each
parameter is a `SqlValue`, named by SQLite storage class: `Null`,
`Integer(Int)`, `Real(Float)`, `Text(String)`, `Blob(Bytes)` or
`Boolean(Bool)`, which binds as `1` or `0`. A value is always bound, never
spliced into the text.

The SQL must be a string literal, or a bare reference to a `String` `const`
bound to one: the first argument of `all`, `first` and `run`, and the `sql` of
every `SqlStatement`. A constant's value is fixed at compile time and can carry
no run-time input, so a query used in several places can be declared once
(`const CountSql: String = "SELECT count(*) AS n FROM deliveries"`). That
covers a constant bound to a `"""` block, one that names another constant, and
one imported from a commons. A parameter, a `let` binding, an interpolated
string, `"a" + B` (not a constant at all) or any other expression is
`bynk.types.sql_not_literal`, so request
data can't reach the SQL text: write `?1` and pass `Text(id)`. A long
statement reads best as a multi-line string, `"""…"""`, which strips the
closing delimiter's indentation and takes no interpolation holes here (a hole
makes it an interpolated string). Tag it `"""sql` and editors highlight the
statement as SQL; the tag is not part of the value.
A `SqlStatement` stays in the code that built it: it can't be a service
parameter or result, a record or event field, an agent store field or a `Json`
target (`bynk.types.sql_statement_at_boundary`), because one that came from
outside would carry SQL that was never a literal. Send the values instead and
build the statement where it runs.

Rows come back as JSON text, which you decode with `Json.decodeSql` into a
record of your own. It's
[`Json.decode`](/book/reference/types/#the-json-codec) for SQL rows: SQLite has
no boolean, so a flag column arrives as `0` or `1`, and `Json.decodeSql` reads
those two numbers as `false` and `true` at a `Bool` field:

```bynk,fragment
type Order = { id: String, total: Int, paid: Bool }

service api from http {
	on GET("/orders/:id") (id: String) -> Effect[HttpResult[Order]] by Visitor given Orders {
		let row <- Orders.first("""sql
			SELECT id, total, paid
			FROM orders
			WHERE id = ?1
			""", [Text(id)])
		match row {
			Ok(Some(text)) => match Json.decodeSql[Order](text) {
				Ok(order) => Ok(order)
				Err(_) => ServerError("the order did not decode")
			}
			Ok(None) => NotFound
			Err(_) => ServerError("the query failed")
		}
	}
}
```

- `all(sql, params)` gives every row as a JSON array of objects keyed by
  column name, for `Json.decodeSql[List[Row]]`.
- `first(sql, params)` gives the first row as one object, or `None` when there
  is none.
- `select[T](tail, params)` reads every row into a `List[T]`, and
  `firstAs[T](tail, params)` the first into an `Option[T]`, with the column list
  written from `T` (see below).
- `run(sql, params)` executes a write and gives a `SqlRun { changes, lastRowId
  }`: the rows it changed, and the rowid of the last row it inserted.
- `batch(statements)` runs a `List[SqlStatement { sql, params }]` in order, in
  one transaction: every statement applies, or none does and the result is the
  failing one's error. It gives one `SqlRun` per statement.

A failed statement is never a fault. Every operation gives a `Result` whose
error is a `SqlError`: `Constraint(message)` for a UNIQUE, CHECK, NOT NULL or
foreign-key violation, which a handler answers (a duplicate key is typically a
`Conflict`), or `Failed(message)` for anything else, with the database's
message. `SqlError` is extensible, so a `match` over it needs a wildcard.

`Json.decodeSql[T]`'s `T` is a row: a record whose fields are `Int`, `Float`,
`String`, `Bool` or `Bytes`, a refined or opaque type over one, or an `Option`
of either (`NULL` is `None`), or a `List` of such records. Anything else is
`bynk.types.sql_row_shape`. Only `0` and `1` become booleans: a `Bool` column
holding `2` still fails the decode. `Json.decode` itself is unchanged and
still refuses `0`/`1` for a `Bool`, at every boundary.

#### Typed reads: `select[T]` and `firstAs[T]` {#d1-typed-reads}

`select[T]` and `firstAs[T]` read rows straight into a record, so the `SELECT`
column list isn't spelt twice. The first argument is the statement **from
`FROM` on**, a string literal (or such a constant) that must not begin with `SELECT`
(`bynk.types.select_tail_has_select`); the compiler writes `SELECT` and `T`'s
fields as quoted identifiers, in declaration order, and prepends them at compile
time, so the SQL is still a constant:

```bynk,fragment
type Activity = { id: String, event: String, receivedAt: Int }

service feed {
	on call(repo: String) -> Effect[Result[List[Activity], SqlReadError]] given Orders {
		Orders.select[Activity]("FROM deliveries WHERE repo = ?1 LIMIT 50", [Text(repo)])
	}
}
// runs: SELECT "id", "event", "receivedAt" FROM deliveries WHERE repo = ?1 LIMIT 50
```

A field selects the column of the same name; there is no renaming, so a computed
column stays with `all` and `Json.decodeSql`. The tail is a literal
(`bynk.types.sql_not_literal`) and `T` is a row, as for `Json.decodeSql`
(`bynk.types.sql_row_shape`); `T` is the row itself, so `select[List[Row]]` is
refused.

`select` gives `Result[List[T], SqlReadError]` and `firstAs`
`Result[Option[T], SqlReadError]`. `SqlReadError` is `Sql(SqlError)` for the
statement's own failure and `Decode(DecodeError)` for a row that did not decode,
such as a column whose stored value disagrees with its field.

Three things about the JSON a row becomes:

- A `BLOB` column arrives as standard base64, so a `Bytes` field decodes it.
- A flag column arrives as `0` or `1`, which `Json.decodeSql` reads at a `Bool`
  field.
- An integer past ±2^53 cannot be represented exactly in JSON, so a field that
  reads one fails the decode rather than rounding.

#### Schema migrations

A database's schema lives in the project's `migrations/` folder, one directory
per database, named by its database name, holding numbered SQL files:

```text
migrations/
  shop-cart-orders/
    0001_create_orders.sql
    0002_add_note.sql
```

A file is `NNNN_<name>.sql`: four digits, an underscore, then letters, digits,
`_` or `-`. They apply in number order, each once, in its own transaction.
The database records which have run in its own `d1_migrations` table, so a
file that has already been applied is never applied again, even if you edit it.
To change the schema, add a new file. Neither D1 nor Bynk has down migrations.

The build checks the folder. It skips hidden files and directories
(`.DS_Store`, an editor's swap file, a `.archive/`) and editor backups ending
in `~`, and copies only what it checked:

- A file that isn't `NNNN_<name>.sql`, isn't directly in a database's
  directory, or shares its number with another is
  `bynk.d1.migration_name`.
- A file that can't be read, or isn't UTF-8, is `bynk.d1.migration_unreadable`.
  So is a symbolic link: links aren't followed, and a skipped migration would
  be a schema change that never runs.
- A directory that matches no `D1` instance is the warning
  `bynk.d1.migrations_unused`. That usually means a capability was renamed,
  and a renamed capability is a new, empty database.

#### SQL checked against the migrations

`bynk check` (and the editor) apply a database's migrations, in order, to an
empty in-memory SQLite and `prepare` each literal statement of that database
against it. Nothing runs. At the literal it reports:

- `bynk.d1.sql_invalid`: a syntax error, or an unknown table or column. The error points
  at the token SQLite refused inside an inline literal (the whole argument for a `const`
  or when the place can't be told);
- `bynk.d1.sql_param_count`: the SQL uses a different number of parameters than
  the list passes. The count is SQLite's own (the highest `?N`, a bare `?` the
  next one), the rule D1 refuses a mismatch by. It is checked when the list is a
  list literal, or a name `let`-bound to one in scope;
- `bynk.d1.select_field_missing`: a `select[T]` / `firstAs[T]` row type has a
  field with no column of that name; and the same for `Json.decodeSql[T]` (or
  `[List[T]]`) of the rows of a literal `all` / `first`, for each required field of
  `T` (an `Option` field with no column is absent, which decodes to `None`, so it
  isn't reported, nor is an extra column). The decode is linked to the statement
  when its text is followed, by name, from `let rows <- Cap.all(…)` through
  `match rows { Ok(text) => … }` (`Ok(Some(text))` for `first`) or the matching
  `let Ok(text) <- … else`; a copy, a helper `fn`, a field or a rebinding of the name
  is not followed, and is not checked.

A migration that doesn't apply is the warning `bynk.d1.migration_invalid`, and
that database's statements are then not checked. A database with no migrations
directory has no schema, so `sql_invalid` and `select_field_missing` don't apply to it; its
parameter counts are still checked, since they depend only on the SQL text and the list
(`:name` and `@name` parameters count once each, as in the test tier). The statements of a `batch` are checked when each is
written inline in the list (`batch([SqlStatement { sql: "…", params: […] }, …])`); one built
elsewhere isn't tied to a database. Statements in a context-level `fn … given Orders` are checked like a handler's. Statements in the `case` and `property`
bodies of a suite (`unit` and `integration` tier) are checked against the suite target's
databases. A `system` suite (`suite … as system { … }`) can't call a capability directly,
and a suite helper `fn` can't hold a D1 call (an effectful helper is
`bynk.suite.fn_effectful`), so neither has a statement to check. Not yet
checked: a list that is neither a literal nor a `let` bound to one. A program that creates tables
at request time, or runs DDL (such as a `CREATE TABLE` without `IF NOT EXISTS` for a
table a migration already made) that SQLite refuses at prepare time, can turn the
check off:

```toml
[check]
d1_sql = false
```

Where they run:

- **Tests:** every case's database starts with its migrations applied, at
  every tier, so a case sees the schema production has. A migration that
  fails fails each case that uses the database, naming the file.
- **`bynk dev`:** they're applied to the local database before the Worker
  serves, and a new file is applied when you save it, without a restart. With
  `-- --remote` nothing is applied: only `bynk deploy` changes a real
  database.
- **`bynk deploy`:** pending ones are applied after the database is bound and
  before the new code is uploaded. If one fails, nothing is uploaded, and the
  previous code keeps serving. See
  [Deploy to Cloudflare](/book/guides/projects-build-and-deployment/deploy-to-cloudflare/#d1-databases).

For a moment during a deploy the old code serves against the new schema, so
keep each migration compatible with the code before it. Add a column with a
default, deploy, and only drop what the old code read in a later migration
(expand, then contract).

#### Testing and stability

In tests each instance is a real SQLite database in memory (Node's built-in
`node:sqlite`, so Node 22.13 or later), so statements run as they do on D1.
`stub Orders.all(_, _)` and `expect Orders.run called` work as for any
capability. `D1` is stable, so it needs no `[unstable] allow`. `bynk deploy`
creates each database and records its id.

### Edge caching: `EdgeCache`

`@cache` on its own only sets `Cache-Control`: Cloudflare doesn't store a
response a Worker generates because of that header. `EdgeCache` turns on
Cloudflare's edge cache (Workers Cache) for the context's Worker, and purges it.
A context declares at most one instance (a second is
`bynk.cloudflare.edge_cache_duplicate`), and a bare `given EdgeCache` is
refused (`bynk.capability.instance_only`):

```bynk,fragment
consumes bynk.cloudflare { EdgeCache }

capability Edge = EdgeCache
```

The instance has no binding. Declaring it emits `[cache] enabled = true` in the
Worker's `wrangler.toml`. A context without one emits exactly what it did before.

An opted-in Worker answers as follows:

| Response | `Cache-Control` sent |
|---|---|
| a `2xx` from a `GET` with `@cache` | the route's, as without `EdgeCache` |
| any other response to a `GET` or `HEAD` (no `@cache`, or not a `2xx`, a `404` included) | `no-store` |
| a `304` | copied from the `200` it stands for |
| any other method | unchanged (the cache stores only `GET` and `HEAD`) |

Cloudflare serves a cached response without running the Worker until its
`max-age` runs out. Only `scope: public` is shared there, and it is shared
even when the request carried `Authorization` (`public` allows that, RFC 9111
§3.5). The cache keys a response by its URL, so a `@cache(scope: public)`
route must answer every caller alike. One whose `by`
names an actor that authenticates is
`bynk.cloudflare.edge_cache_public_authenticated`, so use `scope: private`
there. A Worker that negotiates its `Locale` from `Accept-Language` sends
`Vary: accept-language` on its `@cache` routes, and the edge keeps one
response per language.

`purge(prefixes)` removes every cached response whose path starts with one of
`prefixes`, everywhere. It gives `true` once Cloudflare has accepted the purge
and `false` when it refused one; Cloudflare rate-limits purges at its Free-tier
limits on every plan, so the caller retries. Each prefix is a path starting
with `/`, with no query, fragment or control characters, and anything else
faults. An empty list purges nothing and gives `true`. A content-addressed path
has a fixed length, so purging it as a prefix removes exactly that response:

```bynk,fragment
on POST("/takedown/:hex") (hex: String) -> Effect[HttpResult[Bool]] by v: Visitor given Edge {
	let purged <- Edge.purge(["/dl/sha256/\(hex)"])
	Ok(purged)
}
```

A purge reaches the cache only from the Worker's own `fetch`, so `Edge` is
used from service handlers and the providers and functions they call. An agent
handler that reaches it, directly or through a provider's `given`, is
`bynk.cloudflare.edge_cache_in_agent`, because an agent runs in a Durable
Object. A queue consumer that reaches it is
`bynk.cloudflare.edge_cache_in_queue`, and an `on schedule` handler is
`bynk.cloudflare.edge_cache_in_schedule`: Cloudflare accepts a cron run's purge
but it clears nothing. An exported capability whose provider
reaches it is `bynk.cloudflare.edge_cache_exported`: a consuming context would
build that provider in its own Worker. Export a service that purges instead.

In tests, each instance's double accepts every purge.
`stub Edge.purge(_) returns false` refuses one, and
`expect Edge.purge called with prefixes == ["/dl/sha256/abc"]` checks what was
purged. Under `bynk dev` there is no edge cache: every request runs the
Worker, and `purge` gives `true` after logging
`bynk: EdgeCache.purge is a no-op under bynk dev`. `EdgeCache` is
experimental: list `"bynk.cloudflare.EdgeCache"` under `[unstable] allow`. See
[Cache responses at Cloudflare's edge](/book/guides/effects-and-capabilities/cache-at-the-edge/).

> **Note** — *producing* queue messages (`send`/`sendBatch`) is not yet a
> first-party capability. *Consuming* a queue is a separate entry-point feature:
> see [Process a queued message](/book/guides/entry-points/queue/) and the
> [Queue reference](/book/reference/queue/).

## Cryptography — `bynk.crypto`

`consumes bynk.crypto { Crypto }` brings in digests, HMAC, constant-time
comparison, and signing and verification with imported keys. The unit is
**experimental**: list `bynk.crypto` under [`[unstable] allow`](/docs/manifest/#unstable)
to use it. It runs on all three platforms, over `crypto.subtle` and nothing else.
No third-party crypto runs and nothing is polyfilled.

| Operation | Result |
|---|---|
| `digest(algorithm: Digest, data: Bytes)` | `Effect[Bytes]` — SHA-256, SHA-384 or SHA-512. |
| `hmac(algorithm: Digest, key: Bytes, data: Bytes)` | `Effect[Bytes]`. The key must not be empty. |
| `verifyHmac(algorithm: Digest, key: Bytes, data: Bytes, mac: Bytes)` | `Effect[Bool]`, compared in constant time. |
| `equal(a: Bytes, b: Bytes)` | `Effect[Bool]`, compared in constant time for equal lengths. |
| `importSigningKey(algorithm: Signature, pkcs8: Bytes)` | `Effect[Result[SigningKey, CryptoError]]`. |
| `importVerifyKey(algorithm: Signature, key: Bytes)` | `Effect[Result[VerifyKey, CryptoError]]` — SPKI, or the 32 raw bytes of an Ed25519 key. |
| `importVerifyJwk(jwk: String)` | `Effect[Result[VerifyKey, CryptoError]]` — the algorithm comes from the key's `kty` and `crv`. |
| `sign(key: SigningKey, data: Bytes)` | `Effect[Bytes]`. |
| `verify(key: VerifyKey, data: Bytes, signature: Bytes)` | `Effect[Bool]`; a malformed signature is `false`. |

`Digest` is `Sha256 | Sha384 | Sha512`. `Signature` is `Ed25519 |
EcdsaP256Sha256 | RsaPkcs1Sha256`: ECDSA over P-256 with SHA-256, signing the raw
`r || s` form, and RSASSA-PKCS1-v1_5 with SHA-256. The set is what all three
platforms support. `CryptoError` is `InvalidKey(reason)` or
`UnsupportedAlgorithm(name)`, and carries no key material. All three are
[extensible](#extensible-enums), so a `match` over them needs a wildcard.

**Effects, not pure functions.** `crypto.subtle` is asynchronous on every
platform, so every operation is an `Effect` and there is no hashing in pure
functions, refinements or invariants. Encodings (`toHex`, `toBase64`,
`toBase64Url` and their decoders) stay pure on `Bytes`.

**Keys are handles.** `SigningKey` and `VerifyKey` are opaque: the platform holds
the key non-extractable, so its bytes can't be read back. Like a `Stream`, a key
lives for the call that imported it. It can't be stored, serialised, logged or
sent, so using one in a `store`, a record or event field, a handler parameter or
result, or a wire type is `bynk.types.key_at_boundary`. Keep the key's bytes in
[`Secrets`](#the-portable-surface--bynk) and import them where you sign or verify.

**Timing.** `verifyHmac`, `verify` and `equal` don't return early on the first
differing byte, so they are the way to compare a secret, a MAC, a signature or a
token's digest. `equal` uses the platform primitive where there is one
(`crypto.subtle.timingSafeEqual` on Workers, `crypto.timingSafeEqual` on Node),
and in a browser it HMACs both values under a fresh random key and compares the
results with `subtle.verify`. `Bytes.compare` and `==` are not constant-time and
must never compare secrets. `digest`, `hmac` and `sign` run in time that doesn't
depend on the data in a way a program can exploit.

**The browser.** Web Crypto exists only in a secure context (HTTPS or
`localhost`). Elsewhere an operation fails loudly, naming the requirement; it is
never replaced by something weaker. An algorithm a platform lacks, such as
Ed25519 in an older browser, is an `Err(UnsupportedAlgorithm("Ed25519"))`, by
name.

**In tests,** a unit-tier case runs the real cryptography, because digests, MACs
and signatures are deterministic for given inputs; `stub Crypto.digest(_, _)
returns …` and `expect Crypto.digest called once` work as usual. `Random.bytes`
is deterministic under test, like `Random.int`.

Not in this unit: encryption and decryption, key generation and export, key
derivation, JWT and JOSE helpers, and X.509. The runtime's Bearer and OIDC
verifiers are unchanged. See [Sign and verify data](/book/guides/effects-and-capabilities/sign-and-verify-data/).

## Consuming a first-party capability

Consume the unit, then grant the capability with `given` in each handler that
calls it — exactly as you would a capability you declared yourself:

```bynk
context greeter

consumes bynk { Clock, Logger }

service api from http {
	on GET("/now") () -> Effect[HttpResult[Int]] by Visitor given Clock, Logger {
		let t <- Clock.now()
		let _ <- Logger.info("checked the clock")
		Ok(t.toEpochMillis())
	}
}
```

The Cloudflare surface is consumed the same way — `consumes bynk.cloudflare { Kv }`
and `given Kv` — with the portability trade-off noted above. To configure a
first-party capability (an API key for `Fetch`, say), read it from `Secrets`
rather than passing it as an argument; [Wrap a library as an adapter](/book/guides/effects-and-capabilities/wrap-a-library/)
shows the pattern.

## Stability

Every first-party item, whether a unit, an item it exports or a capability
operation, is **stable** or **experimental**.

- A **stable** item is under the compatibility promise. The kernel and the
  `bynk.*` commons change only at a major release. A capability (in `bynk` or
  `bynk.cloudflare`) only grows within a major version: it may gain items and
  operations, but an existing one is removed or reshaped only at a major
  release, after a deprecation warning. The one exception: a `bynk.cloudflare`
  item may change in a minor release when Cloudflare withdraws or changes the
  feature under it, and that release's notes say so.
- An **experimental** item carries no promise. To use one, list it under
  [`[unstable] allow`](/docs/manifest/#unstable) in `bynk.toml`.

The experimental items today are `bynk.cloudflare`'s `Service`, `R2`,
`RateLimit`, `EdgeCache`, `Queue` and `Analytics`, and `Kv`'s `getWith`,
`putWith`, `getBytes`, `putBytes` and `listPage`; `bynk`'s `Fetch.sendWith`,
`receiveBytes`, `sendBytes`, `receiveStream` and `sendStream`, and
`Logger.log`; and the whole of `bynk.crypto`, which graduates once the
first-party registry has signed with it in production. New platform
capabilities may arrive experimental first, as `D1` did.

### Extensible enums

`FetchError` and `Method`, and `bynk.crypto`'s `Digest`, `Signature` and
`CryptoError`, are **extensible**: they may gain variants in a minor release. Listing every variant they have today is not enough, so a `match` that
covers one, at any depth, needs a wildcard:

```bynk,fail=bynk.types.extensible_match_needs_wildcard
context app.probe

consumes bynk { Fetch }

service probe {
	on call(url: String) -> Effect[String] given Fetch {
		let res <- Fetch.send(Request {
			method: Get,
			url: url,
			contentType: None,
			authorization: None,
			headers: Map.empty(),
			body: None,
		})
		match res {
			Ok(r) => Effect.pure("ok"),
			Err(Network) => Effect.pure("network"),
			Err(Timeout) => Effect.pure("timeout"),
			Err(InvalidHeader) => Effect.pure("header"),
		}
	}
}
```

Add the wildcard, `Err(_)` here, and the `match` keeps compiling when a new
variant arrives:

```bynk
context app.probe

consumes bynk { Fetch }

service probe {
	on call(url: String) -> Effect[String] given Fetch {
		let res <- Fetch.send(Request {
			method: Get,
			url: url,
			contentType: None,
			authorization: None,
			headers: Map.empty(),
			body: None,
		})
		match res {
			Ok(r) => Effect.pure("ok"),
			Err(Timeout) => Effect.pure("retry later"),
			Err(_) => Effect.pure("failed"),
		}
	}
}
```

An enum of your own with the same name is not extensible; the rule follows the
first-party enum, not the name.

## Related first-party modules

Beyond capabilities, the `bynk` namespace also ships pure **commons** you bring
in with `uses` (not `consumes`) — `bynk.list`, `bynk.map` and `bynk.set`
(combinators over the `List`/`Map`/`Set` kernels; `bynk.set` has `union`,
`intersect`, `difference`, `isSubsetOf`, `filterSet` and `mapSet`),
`bynk.string` (string helpers) and `bynk.url` (URLs and query strings, below).
These are ordinary functions with no effects; see the
[type system reference](/book/reference/types/).

`bynk.list` also has three `Ord`-bounded free functions, called unqualified after
`uses bynk.list` (they do not collide with the `xs.min(key)` kernel methods):

| Function | Returns | Notes |
|---|---|---|
| `sort[T: Ord](xs: List[T])` | `List[T]` | ascending and stable, in the type's own [order](/book/reference/types/#ordering-your-own-types) |
| `min[T: Ord](xs: List[T])` | `Option[T]` | the smallest element; `None` for an empty list |
| `max[T: Ord](xs: List[T])` | `Option[T]` | the largest element; `None` for an empty list |

```bynk,fragment
commons demo.sorting

uses bynk.list

type Priority = enum { Low, Medium, High } derives Ord

type Task = {
	priority: Priority,
	title: String,
} derives Ord

fn ranked(ts: List[Task]) -> List[Task] {
	sort(ts)
}

fn hottest(ts: List[Task]) -> Option[Task] {
	max(ts)
}
```

On `Float` they use the total order (`NaN` last).

### URLs and query strings: `bynk.url`

`uses bynk.url` brings in a `Url` record and functions over it, written in Bynk
over two `String` kernel methods, `percentEncode()` and `percentDecode()`.

```bynk
type QueryParam = { name: String, value: String }
type Url = {
  scheme: String,            // lower case
  host: String,              // lower case
  port: Option[Int],
  path: String,              // as written: percent-encoded, "" or starting with /
  query: List[QueryParam],   // decoded, in order, repeats kept
  fragment: Option[String],  // decoded
}
```

- `parse(s) -> Option[Url]` reads an absolute URL,
  `scheme://host[:port][path][?query][#fragment]`, after RFC 3986, not the
  WHATWG URL standard: nothing is repaired. A user-info part (`user:pw@`), an
  IPv6 literal, a missing `//`, a port over 65535, a malformed escape, or a
  path character RFC 3986 doesn't allow unencoded (a space, a non-ASCII
  character) is `None`. In the query, `+` reads as a space.
- `format(u) -> String` writes it back, percent-encoding the query and the
  fragment and writing the path as given. `parse(format(u))` is `Some(u)` for
  every `u` that `parse` returned.
- `formatQuery(params)` and `parseQuery(s)` work on a query string alone.
  `parseQuery` drops a leading `?` and skips a pair it can't decode;
  `decodeQuery(s)` is the strict form, `None` if any pair is malformed.
- `param(u, name)`, `params(u, name)`, `withParam(u, name, value)`,
  `withoutParam(u, name)` and `segments(u)` (the decoded path segments) cover
  the usual edits. A `Url` is a record, so `Url { ...u, path: "/v2" }` works
  too.
- `isScheme`, `isHost`, `isPath`, `parsePort` and `decodePair` are the pieces `parse` is
  built from. They were public before a commons could declare `private`
  helpers, and they stay public so code that calls them keeps compiling.

A `Fetch` caller usually needs only `formatQuery`:

```bynk,fragment
let q = formatQuery([QueryParam { name: "q", value: term }])
let res <- Fetch.send(Request { method: Get, url: "https://api.test/search?".concat(q), contentType: None, authorization: None, headers: Map.empty(), body: None })
```

`Url` is a record in a commons, not a built-in type, so a project's own `Url`
type is untouched unless it also writes `uses bynk.url`.

Two more serve the `Locale` capability above.
**`bynk.locale.types`** is a dependency-free leaf declaring `LocaleTag`,
`Message` and `MessageArg` — `uses` it wherever you only need to *name* those
types, which is all a context calling `Locale.current()` requires.
**`bynk.locale`** declares the value-level API: the `message`/`withText`/
`withWhole`/`withNum`/`withMoment` builders and the bundle-free `render`
fallback. A commons declaring a `messages` bundle needs both; a context
consuming that bundle should `uses` only `bynk.locale.types`, since
`bynk.locale` and every message-bundle commons both export a `render`. See
[Localisation](/book/guides/localisation/).

> **Deprecated (v0.91, ADR 0116 D6):** the `bynk.list` free functions whose method
> forms now exist — `map`, `filter`, `find`, `any`, `all` — emit a non-failing
> `bynk.list.deprecated_function` warning at each call site (the build still
> succeeds) with a machine-applicable fix to the method form: `map(xs, f)` →
> `xs.map(f)`, `find(xs, p)` → `xs.filter(p).first()`, and so on. Prefer the
> [`List` methods](/book/reference/types/#list-methods). `reverse` and `traverse` keep their
> free-function form (no method equivalent yet).

**See also:** [Capabilities & providers](/book/reference/capabilities/),
[Adapters](/book/reference/adapters/),
[Understand the capability model](/book/guides/effects-and-capabilities/understand-the-capability-model/).
