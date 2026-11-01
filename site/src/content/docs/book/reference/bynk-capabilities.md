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
| **`Idempotency`** | `dedup[T](key: String) -> Effect[Option[T]]` · `remember[T](key: String, value: T, expiresAfter: Duration) -> Effect[()]` — mechanical dedup for at-least-once delivery. See below. |
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
error. The shipped provider is a single in-memory map, identical on every platform: state
is lost on process restart, and there is no durability or multi-instance sharing yet — a
durable, platform-native provider is a named but unfiled future direction (see
[the track's own settling notes](https://github.com/bynk-lang/bynk/blob/main/design/tracks/idempotency-capability.md)
for why that's a separate axis from portability, not a variant of it).

The `key` you pass is automatically scoped to the calling handler's own qualified name
before it reaches the provider — `Idempotency.dedup[T]("order-1")` inside `on call` of
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

exports transparent { PaymentConfirmed }
consumes bynk { Events }

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
  timeout:  Option[Duration],
  redirect: Redirect,
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

- `timeout` bounds the whole exchange, the response body included, except for
  the [streamed operations](#streamed-bodies), where it bounds the response
  head only. When it elapses the result is `Err(Timeout)`. A timeout of zero or less, or over
  2,147,483,647 ms (about 24 days, the longest a platform timer takes),
  faults the call, with nothing sent. `None` waits as long as `send` does.
- `redirect: Follow` follows redirects, as `send` does. `Manual` returns the
  `3xx` itself, with its `location` header, so the service decides whether to
  follow it (to refuse a redirect to an internal host, say).

The options are a record of their own, not fields of `Request`, because adding a
field to `Request` would break every request literal: record fields have no
defaults. `sendWith` is experimental: list `"bynk.Fetch.sendWith"` under
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

`Kv` is backed by a Worker KV namespace bound as `env.KV`; the
`[[kv_namespaces]]` stanza is derived for you.

### Several namespaces: derived capabilities

For more than one namespace, declare each as a derived capability:
`capability Sessions = Kv` is a distinct instance of `Kv`, with `Kv`'s
operations and its own namespace, bound as `env.KV_SESSIONS`. A context may use
the bare `Kv` alongside its instances, and the bare `KV` stanza is emitted only
when it does. See [Derived capabilities](/book/reference/capabilities/#derived-capabilities-capability-x--k).

### Metadata, expiry and binary values

Five more `Kv` operations reach what the platform has beyond text. They are
experimental: list each one you use under `[unstable] allow`
(`"bynk.cloudflare.Kv.putWith"` and so on). The existing operations are
unchanged.

```bynk
type KvExpiration = | Never | After(ttl: Duration) | At(instant: Instant)
type KvPut      = { expiration: KvExpiration, metadata: Map[String, String] }
type KvGet      = { cacheTtl: Option[Duration] }
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

A request body is still read whole, so a client's upload doesn't stream into
R2; send a large object as a multipart upload, a part a request.

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

Text that is not JSON faults the send, and so does going over the platform's
caps: 128 KB a message, and 100 messages or 256 KB a batch. Delivery is at least
once and the platform stores the messages, so make consumers idempotent and keep
secrets out of bodies. Nothing checks that a producer's message matches the
consumer's message type: the body is text, encoded by the caller.

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

The SQL must be a string literal: the first argument of `all`, `first` and
`run`, and the `sql` of every `SqlStatement`. An interpolated string, a
variable or any other expression is `bynk.types.sql_not_literal`, so request
data can't reach the SQL text: write `?1` and pass `Text(id)`. A long
statement is one long literal, since there's no multi-line string literal.
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
		let row <- Orders.first("SELECT id, total, paid FROM orders WHERE id = ?1", [Text(id)])
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
capability. `D1` is experimental: list `"bynk.cloudflare.D1"` under
`[unstable] allow`. `bynk deploy` creates each database and records its id.

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
`RateLimit`, `EdgeCache` and `D1`, and the
whole of `bynk.crypto`, which graduates once the first-party registry has signed
with it in production. New platform capabilities may arrive experimental first.

### Extensible enums

`FetchError` and `Method`, and `bynk.crypto`'s `Digest`, `Signature` and
`CryptoError`, are **extensible**: they may gain variants in a minor release. Listing every variant they have today is not enough, so a `match` that
covers one, at any depth, needs a wildcard:

```bynk,fail=bynk.types.extensible_match_needs_wildcard
context app.probe

consumes bynk { Fetch }

service probe {
	on call(url: String) -> Effect[String] given Fetch {
		let res <- Fetch.send(Request { method: Get, url: url, contentType: None, authorization: None, headers: Map.empty(), body: None })
		match res {
			Ok(r) => Effect.pure("ok")
			Err(Network) => Effect.pure("network")
			Err(Timeout) => Effect.pure("timeout")
			Err(InvalidHeader) => Effect.pure("header")
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
		let res <- Fetch.send(Request { method: Get, url: url, contentType: None, authorization: None, headers: Map.empty(), body: None })
		match res {
			Ok(r) => Effect.pure("ok")
			Err(Timeout) => Effect.pure("retry later")
			Err(_) => Effect.pure("failed")
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
