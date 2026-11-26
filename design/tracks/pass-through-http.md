# Pass-through HTTP — rest segments, forwarded requests and pass-through responses

- **Status:** Settling in the draft PR that adds this doc. Spine issue: #3752,
  which carries the live slice status.
- **Realises:** the service-tier HTTP surface
  (`site/src/content/docs/reference/http.md`) and the outbound streamed bodies
  of `Fetch` (`reference/bynk-capabilities.md`, *Streamed bodies*). It answers
  the question #3752 leaves open, whether pass-through belongs in Bynk at all
  (§4).
- **Posture:** Feature track per
  [ADR 0076](../decisions/0076-feature-track-posture.md), run per
  [ADR 0167](../decisions/0167-feature-tracks-run-github-native.md). It
  qualifies on all three axes (§1.2).
- **Governing records:**
  [ADR 0879](../decisions/0879-reserve-route-syntax.md) (`*`, `{`, `}`
  reserved in routes; this track spends `*`),
  [ADR 0914](../decisions/0914-route-matching-order.md) (route order),
  [ADR 0222](../decisions/0222-http-route-path-escape.md) (path escapes),
  [ADR 0126](../decisions/0126-httpresult-rfc9110-status-vocabulary.md) (the
  closed `HttpResult` vocabulary, left untouched here),
  [ADR 0162](../decisions/0162-http-method-correctness.md) (`HEAD`,
  `OPTIONS`, `405` + `Allow`),
  [ADR 0163](../decisions/0163-http-response-caching.md) and
  [ADR 0590](../decisions/0590-caching-raw-responses.md) (`ETag` and `304`),
  [ADR 0159](../decisions/0159-cors-policy.md) (CORS),
  [ADR 0164](../decisions/0164-http-security-headers.md) and
  [ADR 0954](../decisions/0954-html-response-security-headers.md) (security
  headers),
  [ADR 0165](../decisions/0165-http-request-body-limits.md) (body limits),
  [ADR 0613](../decisions/0613-edge-cache.md) (`EdgeCache`),
  [ADR 0453](../decisions/0453-fetch-headers.md),
  [ADR 0655](../decisions/0655-fetch-options-and-log-levels.md) and
  [ADR 0667](../decisions/0667-fetch-streamed-bodies.md) (the outbound
  `Fetch` surface),
  [ADR 0899](../decisions/0899-seam-consumed-credential-hidden-from-request-headers.md)
  (consumed credentials hidden from handlers),
  [ADR 0821](../decisions/0821-signed-cookie-actor.md) (the `SignedCookie`
  CSRF rule),
  [ADR 1024](../decisions/1024-entry-points-start-a-trace.md) (traces; a
  client's `traceparent` is never believed).
- **Related increments:** #3753 (shipped as ADR 0879), #3825 (CSRF on the
  route that calls `Actor.issue`, accepted, not yet built), #4504 (CORS origin
  shape, shipped; its reader is reused in §7.2), #3895 (traces).
- **Code on `main`:** cited by symbol, never by line, as of `09c6e9a5`.

Every open question is written as a **[DECISION]** with a recommendation and
the alternatives rejected. A decision that is a product call rather than an
engineering one is marked **Maintainer ruling needed**; §16 lists them.

## 1. The theme

### 1.1 What this track settles

Some services answer for a whole subtree of paths rather than a fixed set of
routes: a reverse proxy that puts another site under a path, a file server
over R2 keys that contain `/`, a single-page app's fallback. Today a Bynk HTTP
service can write none of them without a workaround, because a route matches
a fixed number of segments, and every response passes through a typed
boundary that adds its own `ETag`, answers `HEAD` and `OPTIONS` itself, and
can't return a computed status.

The case that surfaced this is the blog. `bynk-lang/blog` is its own Pages
project, served at `bynk-lang.org/blog/` by a TypeScript Worker in that
repository's `proxy/` directory. Its README says why it isn't Bynk, and names
this track. The track ends when that Worker is Bynk (§11).

This doc settles:

1. **Where pass-through belongs** (§4): the ordinary service tier, with a
   distinct route form for pass-through.
2. **A rest segment** (§5): `*name` as a route's last segment, binding the
   raw rest of the path as a `String`. Useful alone.
3. **A pass-through route and response** (§6): `on ANY(…)` returning
   `Effect[PassThrough]`, with every suspended boundary behaviour named.
4. **Forwarding** (§7): an `Upstream` capability instance whose origin is
   declared, never computed, with safe header defaults.
5. **The threat model** (§8), **composition** with what exists (§9),
   **testing** (§10), **acceptance by porting the blog** (§11), **the 1.0
   position** (§12), and **the slices** (§13).

### 1.2 Why a track (the ADR 0076 trigger)

- **Multi-increment.** A route-grammar change, a header view, a new route
  form and response, and a forwarding capability each touch the parser,
  checker, router emitter and runtime separately, and each is useful without
  the next (§13).
- **Surface not yet settled.** Whether Bynk should do this at all, the rest
  syntax, and which boundary behaviours are suspended were all open.
- **Security boundary.** Forwarding is where SSRF, credential leakage,
  cookie scoping and cache poisoning live (§8). Settling found a host-confusion issue in
  the current TypeScript proxy (§2.7), which is the strongest argument for
  doing this in the language rather than leaving it to hand-written Workers.

## 2. What's true today

### 2.1 Routes match whole segments

- A route is `on <METHOD>("<route>")`, where a `:name` segment is one path
  parameter (`reference/http.md` §Handler form). Methods are `GET`, `POST`,
  `PUT`, `PATCH` and `DELETE` (`HttpMethod` in `bynk-syntax/src/ast.rs`,
  `IrHttpMethod` in `bynk-model`, and `http_method` in
  `tree-sitter-bynk/grammar.js`). `HEAD` and `OPTIONS` aren't declarable.
- `*`, `{` and `}` are already reserved in every segment
  (`reserved_route_char` in `bynk-check/src/context_checks.rs`,
  `bynk.http.reserved_route_syntax`, ADR 0879). #3752's Q8 is answered:
  whatever spelling this track picks is additive.
- The runtime matches with `matchPath` in
  `bynk-emit/src/emitter/runtime/http.ts`: it splits both pattern and path on
  `/`, **drops empty segments**, requires equal counts, and compares every
  segment **percent-decoded** (`decodeURIComponent`). So `/items`, `/items/`
  and `//items` are one path (#3909), and `/blog` and `/blog/` reach the same
  route. `decodePath` is the same split for the 400 `MalformedPath` check.
- Route order is `bynk_check::route_order::order` (segment by segment, a
  literal beats a `:name`), applied by `sort_routes` in
  `bynk-emit/src/emitter/workers_entry/http.rs`. Unreachable routes are
  `bynk_check::route_order::shadowed`, reported as
  `bynk.http.unreachable_route`. Both read segments through the private
  `Seg` enum (`Lit` or `Param`).
- `param_count` in `bynk-emit/src/emitter/workers_entry/http_policy.rs`
  counts `:` segments; it drives the sort key and whether a route branch
  binds `__m` from `matchPath`.
- Handler names come from `http_handler_method_name_from_str` (via
  `http_handler_method_name_ir`) in
  `bynk-emit/src/emitter/emit/handlers.rs`: `http_<METHOD>` then one part per
  segment, `Param_<name>` for a parameter, every other character through
  `sanitise_path_segment`, which maps anything not alphanumeric to `_`.
  **Found while settling:** that mapping isn't injective. `GET /a-b` and
  `GET /a_b` both compile and both emit `http_GET_a_b`, so one handler
  answers both paths (checked with `bynk compile --target workers` on
  `09c6e9a5`). It's out of this track's scope and raised separately, but the
  rest-segment naming in §5.6 must not inherit it.
- The Worker's `fetch` dispatches in a fixed order: the internal
  `/_bynk/call/` Service Binding prefix, the admin routes (`INSPECT_PREFIX`),
  WebSocket upgrades, `/_bynk/event/`, then **the CORS preflight**
  (`cors_preflight`, one branch per CORS service), then the user routes
  (`emit_http_route_dispatch`). So a root catch-all can never shadow
  `/_bynk/`, but the CORS preflight *would* intercept a proxied `OPTIONS`
  unless excluded (§6.4).

### 2.2 Responses are typed and mostly 200-only

- `HttpResult` is a closed sum defined by `HTTP_VARIANTS` in
  `bynk-syntax/src/ast.rs` (ADR 0126). `Raw`, `RawTagged` and `RawStreamed`
  are 200-only; `RawCreated` is 201. `RawError` takes a status *named* at the
  call site (`raw_error_status`, `bynk.http.raw_error_status`,
  `bynk.http.error_status_position`), never a computed one. An upstream's
  304, 404 or 308 can't be returned as it came.
- **`HttpResult` is exhaustively matchable and not extensible.** A
  `match` on an `HttpResult[String]` with two arms is
  `bynk.types.non_exhaustive_match`, naming every missing variant
  (checked on `09c6e9a5`). This matters for §12: a new `HttpResult` variant
  breaks such a `match`.
- `withHeader` can't set a header the router owns
  (`RESERVED_RESPONSE_HEADERS` in `runtime/http.ts`):
  `Cache-Control`, `ETag`, `Vary`, `Location`, `Set-Cookie`, the security
  and CORS headers, `Allow` and the hop-by-hop headers among them.

### 2.3 The boundary shapes every response

From `reference/http.md` §Method semantics, §Caching, §CORS, §Security
headers, and the runtime:

- **Weak `ETag`** on an eligible `GET` (`weakETag`, `RAW_WEAK_ETAG_MAX_BYTES`)
  and a router `304` (`notModifiedIfMatch`).
- **`HEAD` from `GET`**: the `GET` route's guard widens to `HEAD`
  (`emit_http_route_dispatch`) and `headResponse` strips the body.
- **`OPTIONS` and `405` with `Allow`**, derived from the route table
  (ADR 0162; `allowUnion`).
- **CORS**: the preflight and `applyCors` stamping (ADR 0159).
- **Security headers**: `applySecurityHeaders` stamps `nosniff` on every
  response, HSTS when declared, and the HTML headers on `text/html`
  (ADR 0164, 0954), overwriting a value already present.
- **Caching**: `Cache-Control: private` on a credentialed route
  (`privateByDefault`), `@cache`, and under `EdgeCache` `edgeCacheGuard`'s
  `no-store` on a `GET`/`HEAD` answer with no `Cache-Control`.
- **Error shapes**: `{ "error": … }`, problem details, and `on rejected`.

Each is right for a typed service and wrong for a proxy, which must not
invent an `ETag` or answer `OPTIONS` itself.

### 2.4 The request is read through narrow views

- `headers: RequestHeaders` has `get`, `bearer` and `clientIp`
  (`RequestHeaders` and `requestHeaders` in `runtime/http.ts`). It **can't be
  enumerated**. A route's consumed credential is hidden from it
  (`hideCookie`, `hideHeaders`; ADR 0899).
- `query: RequestQuery` has `get`, `all` and `params()`. The raw query string
  is held internally (`requestQuerySearch`) but isn't exposed, so a handler
  can't reproduce a query byte for byte (key order across names, `+` against
  `%20`).
- A `@raw` route can take `body: Stream[Bytes]`, read under the route's cap by
  `requestBodyStream`.

### 2.5 The outbound side is mostly there

- `Fetch.receiveStream` and `Fetch.sendStream` return a `StreamResponse`
  (`status`, `headers: Map[String, String]`, `body: Stream[Bytes]`), both
  experimental. Implemented by `FetchProvider.exchange` in
  `bynk-std/lib/core/bindings/bynk-cloudflare.ts`.
- `exchange` refuses `FORBIDDEN_REQUEST_HEADERS` (`host`, `content-length`,
  `connection`, `keep-alive`, `te`, `trailer`, `transfer-encoding`,
  `upgrade`, `expect`) and a repeated name.
- Response headers arrive joined with `", "`, which is lossy for
  `Set-Cookie` (`reference/bynk-capabilities.md`, *Request and response
  headers*).
- `sendStream` never follows redirects; `FetchOptions.redirect: Manual`
  returns the `3xx` with its `location`.
- Every emitted Worker carries `global_fetch_strictly_public`, so `Fetch`
  reaches only the public internet.
- The URL is a `String` built at run time, so nothing stops it being built
  from the request.

### 2.6 Deployment is already fine

`[deploy.workers.<key>] routes = ["bynk-lang.org/blog/*"]` attaches a Worker
to a path (`reference/tooling/manifest.md`, `routes`). The `env(…)` constant
(`reference/constants.md`) gives a per-environment upstream origin.

### 2.7 The reference: the TypeScript blog proxy

`bynk-lang/blog`, `proxy/src/index.ts` (`handle`), with ten tests in
`proxy/test/index.test.ts` and the deploy in `proxy/wrangler.toml`. Every
behaviour it implements is listed in §11.1.

**Found while settling: a host-confusion issue in the current proxy**, in
how it builds the upstream URL from the request path. It is reported and
fixed separately from this PR; the detail is added here once that fix is
deployed. It is the kind of mistake §7.2's design rules out by
construction, and the reason D1 argues against leaving pass-through to
hand-written Workers.

## 3. Goals and non-goals

**Goals.**

- A route can match the rest of a path, with its precedence and edge cases
  settled, in any ordinary service (slice 1).
- A handler can enumerate the request's headers (slice 2).
- A route can pass an upstream's status, headers and streamed body through,
  with every suspended boundary behaviour stated (slice 3).
- Forwarding is safe by default: the upstream is declared, credentials don't
  cross unless asked, and a redirect keeps the browser on the proxy (slice 4).
- The blog proxy is a Bynk Worker, behaviourally equivalent (slice 5).

**Non-goals.**

- **Rewriting bodies.** No HTML rewriting, `Link` or `Content-Location`
  rewriting. The blog is built with its base path, as any proxied site must
  be.
- **WebSocket pass-through.** An `Upgrade` is refused on a pass-through route
  (§7.4). A later track may add it.
- **A general HTTP client change.** `Fetch`'s `Map[String, String]` headers
  stay as they are (§7.1, decision D12).
- **Credential injection** (a proxy adding its own API key for the upstream).
  A natural follow-up, `Upstream @bearer(…)`; not needed for the blog.
- **Load balancing, retries, health checks.** One origin per instance.
- **Rest segments in WebSocket `on open` routes.** Out of scope, as in
  ADR 0879.

## 4. Where it belongs

Three shapes were weighed. What each means for the boundary guarantees every
other route keeps:

| | (A) Ordinary service tier, distinct route form | (B) A `from proxy` service | (C) Adapter-level escape hatch |
|---|---|---|---|
| Surface | `on ANY("/blog/*path")` returning `Effect[PassThrough]`, using an `Upstream` instance | `service blog from proxy { upstream: …, mount: "/blog" }`, no handler | A guide; a hand-written Worker, or a `bynk.toml` knob that routes a path to raw TypeScript |
| Upstream origin | Declared on a capability instance; can't be computed | Declared in the service | Whatever the author writes (today: see §2.7) |
| Actors, rate limits, body caps | The ordinary seams, unchanged | Re-implemented, or absent | Absent |
| Local logic (the `/blog` redirect, auth decisions, error mapping) | Yes | No, or a growing config language | Yes, untyped |
| Boundary behaviours on *other* routes | Unchanged; suspensions scoped to `ANY` routes and stated per behaviour (§6) | Unchanged, but a second router with its own precedence rules beside the first | Unchanged; the proxy is outside Bynk |
| Tests | The ordinary tiers and doubles (§10) | New machinery | None from Bynk |
| Rest segments for R2 and SPAs | Same feature, ordinary routes | Separate | Separate |

**[DECISION D1] Pass-through is part of the ordinary service tier, as a
distinct route form (A).** **Maintainer ruling needed** — this is the "should
Bynk do this at all" question #3752 leaves open.
Recommendation: (A). A pass-through route is a route: it ranks with the
others, takes `by`, `@rateLimit` and `@limit`, is tested like the others, and
the boundary behaviours it gives up are named in one table (§6.3) and visible
in its signature (`ANY`, `Effect[PassThrough]`). The SSRF guarantee doesn't
need a separate service: putting the origin on a capability instance (§7.2)
gives it to (A).
Rejected: **(B)**, because it needs a second router beside the first, with
its own precedence against ordinary routes in the same Worker, and it can't
express the blog's own one piece of logic (`/blog` → `/blog/`) without a
configuration language that grows towards a handler. **(C)**, because the
blog proxy is the evidence: hand-written, reviewed and tested, it still
had a host-confusion issue in how it built the upstream URL (§2.7). Leaving pass-through
outside the language leaves every author to rediscover that.

If the ruling is (C), slice 1 (rest segments) and slice 2 (header
enumeration) still stand, and the track retires with a guide, as #3752's end
state (b) allows.

## 5. The rest segment

### 5.1 Syntax

**[DECISION D2] `*name` as the route's last segment.**
`on GET("/files/*key")`, `on ANY("/blog/*path")`.
Recommendation: `*name`. It's the glyph Cloudflare's own route patterns use
(`bynk-lang.org/blog/*` in the same project's `bynk.toml`), and Rails and
Express 4 spell it the same. It's the shortest form, and it leaves `{` and
`}` reserved for a later in-segment template (`/files/{name}.{ext}`).
Rejected: `:name*` (path-to-regexp), which reads as a modifier on a
parameter and invites `:name+` and `:name?` next; `{*name}` (ASP.NET,
`matchit`), which spends the braces; `{...name}` (Astro), the same.

Rules, each a compile error with a diagnostic of its own, settled in
slice 1:

- **Last only.** `*` anywhere but the start of the last segment stays
  `bynk.http.reserved_route_syntax`.
- **Named.** A bare `*` is refused (`bynk.http.rest_unnamed`); the name
  binds a parameter, as `:name` does.
- **Whole segment.** `/files/x*key` and `/files/*key.txt` stay reserved.
- **One per route.** Follows from "last only".
- **Under `/_bynk/`.** Already `bynk.http.reserved_prefix`; unchanged.
- **Root catch-all allowed.** `on GET("/*path")` is legal. It can't shadow
  `/_bynk/`, which is dispatched first (§2.1).

### 5.2 What it binds

**[DECISION D3] A `String` holding the rest of the path as received: still
percent-encoded, starting with `/` unless empty, trailing slash and empty
segments kept, query excluded.**

For `on ANY("/blog/*path")`:

| Request path | `path` |
|---|---|
| `/blog` | `""` |
| `/blog/` | `"/"` |
| `/blog/posts/money/` | `"/posts/money/"` |
| `/blog/posts/money` | `"/posts/money"` |
| `/blog//x` | `"//x"` |
| `/blog/caf%C3%A9` | `"/caf%C3%A9"` |
| `/blog/a%2Fb` | `"/a%2Fb"` |

The parameter must be typed `String` (`bynk.http.rest_param_type`), and its
name must match a handler parameter as `:name` does.

Recommendation: a raw `String`, because a proxy must send exactly what it
received. Decoding and re-encoding loses distinctions an upstream makes
(`%2F` against `/`, `%20` against `+`, and which characters were escaped),
and dropping empty segments or the trailing slash breaks the blog: Pages
answers `/posts/x` with a redirect to `/posts/x/`, and the proxy must be able
to tell `/blog` (redirect) from `/blog/` (forward). A file server decodes
the key itself with the existing kernel method, `path.percentDecode()`,
which is `None` for a malformed escape.
Rejected: **`List[String]` of decoded segments**, which loses the trailing
slash, empty segments and `%2F`, and so can't drive a proxy. **A `Path`
type** that rules out `..`, because the router already guarantees that
(§5.3) and a new type would be one more thing to learn for one property.
**Accepting both, chosen by the declared type**, which is reasonable later
but doubles slice 1's surface for no current user.

### 5.3 Edge cases

**[DECISION D4] The edge cases.**

- **Percent-decoding.** The segments *before* the rest are matched decoded,
  as today. The rest is never decoded by the router, so a malformed escape in
  it isn't a `400`; it's the handler's to judge. A malformed escape in a
  segment before it is the usual `400 MalformedPath`.
- **Dot segments.** The router's `path` comes from the parsed request URL,
  and WHATWG parsing has already resolved `.`, `..`, `%2e` and `%2e%2e`
  (checked: `/blog/a/../../x` and `/blog/%2e%2e/x` both parse to `/x`). So
  the rest never holds a dot segment, and the matched prefix can't be
  escaped. The router still checks, as defence in depth, and answers
  `400 MalformedPath` if one appears (a host that doesn't normalise, a test
  harness that builds a path by hand).
- **Empty and trailing-slash paths.** Kept in the value (table above).
  Matching is unchanged: the rest segment matches **zero or more** remaining
  segments, so `/blog/*path` matches `/blog` itself. Empty segments before
  the rest are insignificant, as today.
- **The query string.** Not part of the rest. A pass-through route reads it
  raw from the request (§7.1); an ordinary route uses `query`.
- **Backslash.** `\` in a request path is `/` to the WHATWG parser for
  `https`, so it never reaches the rest; nothing further.

Rejected: decoding the rest and refusing `%2F` (breaks keys and upstreams
that use it); matching *one or more* segments (the blog then needs a second
route for `/blog`, and the two interact badly with empty-segment
equivalence: a literal `/blog` route also answers `/blog/`, and would beat
the rest route there, redirecting `/blog/` to itself).

### 5.4 Precedence

**[DECISION D5] A rest segment is the weakest segment.** At the first
position where two routes of a method differ: a literal beats `:name`, and
`:name` beats `*rest`.

- `GET /files/index.json` beats `GET /files/*key` for `/files/index.json`.
- `GET /a/b/*y` beats `GET /a/*x` for `/a/b/c` (at position 2, `b` beats
  `*x`).
- `GET /a/:id` beats `GET /a/*x` for `/a/1`; `/a/1/2` goes to `/a/*x`.
- A rest that matches zero segments still loses at the position where it
  starts: `GET /blog` beats `ANY /blog/*path` for `/blog` (and, because empty
  segments don't count, for `/blog/` too: the reason the blog port uses no
  literal `/blog` route; §11.2).
- `GET /a/*x` beside `GET /a/*y` is `bynk.http.unreachable_route`, as for
  `:name`. `GET /a/:p` beside `GET /a/*x` is not: `/a/*x` still answers `/a`
  and `/a/1/2`, so neither shadows the other. `shadowed` gains the rest case
  in slice 1, with fixtures for each line above.
- **Methods, and who owns a path.** For each request, the router first finds
  the best shape among **all** routes that match its path, whatever their
  method, `ANY` included, by the order above. If that best shape is an
  `ANY` route's, and it is strictly more specific than every ordinary
  route matching the path, the `ANY` route **owns** the request and
  receives it whatever its method. Otherwise ordinary routing applies
  exactly as today: the request's own method among the ordinary routes,
  then `405` with `Allow`, `OPTIONS` and `HEAD` from `GET`, and the CORS
  preflight. `ANY` routes don't take part at all. A tie between an `ANY`
  route and an ordinary route at equal shape goes to the ordinary route.
  So a root catch-all `ANY("/*path")` beside `GET /api/items` leaves
  `/api/items` with its own `405`, `Allow` and preflight, and receives only
  paths no ordinary route matches. A `POST` to a path whose only route is
  `GET /blog/feed.json` is that route's `405`, never forwarded. An `ANY`
  route that some ordinary route always beats is
  `bynk.http.unreachable_route`.

The outcome still never depends on how a route is spelt.

### 5.5 Method semantics on an ordinary rest route

An ordinary `GET /files/*key` keeps every boundary behaviour: `HEAD` from
`GET`, `OPTIONS` and `405` with `Allow`, the weak `ETag`, CORS, security
headers. `allowUnion` and the CORS preflight's path list carry the rest
pattern like any other.

### 5.6 The router and handler naming

**[DECISION D6] What changes in code (slice 1).**

- `bynk_check::route_order`: `Seg` gains `Rest`; `overlaps`, `order` and
  `shadowed` treat it as matching zero or more trailing segments, weakest
  at its position.
- `matchPath` and `decodePath` (`runtime/http.ts`): a pattern ending in
  `*name` matches when the decoded prefix segments match, and binds `name`
  to the raw remainder sliced from the undecoded path, so the value is the
  table in §5.2 exactly. The slice is taken from the raw path, not rebuilt
  from segments.
- `param_count` (`http_policy.rs`) counts a rest as a parameter, so the
  branch binds `__m`. The sort key in `sort_routes` stays "fewest parameters
  first, then `route_order`".
- `http_handler_method_name_from_str` (`emit/handlers.rs`): a rest segment
  becomes `Rest_<name>`, so `GET /files/*key` is `http_GET_files_Rest_key`.
  An `ANY` route is `http_ANY_…`. Given the collision in §2.1, slice 1 adds a
  check that no two handlers of a service share an emitted name, refusing
  one that would, rather than relying on the scheme.
- `bynk client` and `bynk openapi` leave a rest route out, named on stderr,
  as for the routes they already leave out
  (`reference/http-client.md` §Routes left out). OpenAPI path parameters
  can't contain `/`, and a client that took a rest as one segment would
  encode its slashes.
- The test address is the pattern then the rest:
  `files.GET("/files/*key", "/a/b.png")`.

## 6. The pass-through route and response

### 6.1 The route form

**[DECISION D7] `on ANY("<route>")` declares a pass-through route. It must
return `Effect[PassThrough]`, and only it may.** **Maintainer ruling
needed** — this fixes the surface the track is built on.

```bynk,ignore
on ANY("/blog/*path") (path: String, request: InboundRequest)
    -> Effect[PassThrough] by Visitor given Pages { … }
```

- `ANY` answers every method the platform delivers, `HEAD` and `OPTIONS`
  included, because the upstream has its own method contract. Methods
  outside `GET HEAD POST PUT PATCH DELETE OPTIONS` (`TRACE`, `CONNECT`,
  extensions such as `PROPFIND`) are answered `501 Not Implemented` by the
  router and never forwarded; `TRACE` especially, which would reflect the
  request's credentials.
- `ANY` with a return type other than `Effect[PassThrough]`, or
  `Effect[PassThrough]` on another method, is an error
  (`bynk.http.pass_through_signature`).
- Its parameters are path parameters, a rest, `request: InboundRequest`,
  `cf: RequestCf`, and nothing that reads the body (`body`, `@raw`, `@form`,
  `@multipart` are refused): the body belongs to the upstream.
- `ANY` is new syntax in the method position: `HttpMethod` and
  `IrHttpMethod` gain a variant, and so does `http_method` in
  `tree-sitter-bynk/grammar.js` and `vscode-bynk`'s grammar.

Rejected: **a `Forward` variant on `HttpResult`** (#3752's sketch). It
breaks every exhaustive `match` on `HttpResult` (§2.2, §12), and it would let
any `GET` route return an upstream response through a boundary that still
synthesises `HEAD` and `ETag` around it, so the suspensions would depend on
the value at run time, not on the route. **`@passThrough` on an ordinary
method**, because a proxy must receive `HEAD` and `OPTIONS`, which aren't
declarable, and one annotated `GET` beside an unannotated `POST` at the same
path would split one upstream across two contracts. **Allowing `ANY` with
`HttpResult`** (a catch-all typed route), which has no user and blurs which
routes keep `405`.

### 6.2 The response type

**[DECISION D8] `PassThrough` is a built-in sum: `Forward(Forwarded)` or
`Respond(HttpResult[()])`.**

- `Forward(response)` sends an upstream's answer through (§6.3).
  `Forwarded` is opaque, produced only by `Upstream.forward` (§7.2): there is
  no constructor, so a handler can't fabricate an upstream response, and it
  can't carry a status or header Bynk didn't receive from the declared
  origin. It has read-only `status() -> Int` and
  `header(name) -> Option[String]` for a handler that decides on them.
- `Respond(result)` answers locally with any `HttpResult[()]` variant
  (`PermanentRedirect`, `BadGateway(…)`, `NotFound`, `RawError(…)`), shaped by
  the boundary exactly as on an ordinary route, except CORS (§6.4).

So the blog's whole handler is one `if` and one `match` (§11.2).

The new variant names resolve by expected type: a program with its own
`Forward` or `Respond` variant keeps compiling (checked against the same
situation with a user `Found` beside `HttpResult.Found`).

Rejected: **`Result[Forwarded, HttpResult[()]]`**, which reads a redirect
as an error; **a separate `Proxied` type per handler**, for no gain;
**`Forward(StreamResponse)`** from `Fetch.receiveStream`, which re-admits a
URL built at run time (§8) and loses repeated headers through its `Map`.

### 6.3 What a `Forward` response suspends and keeps

**[DECISION D9] The behaviours, one by one.**

| Behaviour | On `Forward(…)` | Why |
|---|---|---|
| Weak `ETag` (`weakETag`) | **Suspended.** The upstream's `ETag`, strong or weak, passes as sent. | A proxy must not invent a validator, and hashing would buffer the stream. |
| Router `304` (`notModifiedIfMatch`) | **Suspended.** `If-None-Match` and `If-Modified-Since` are forwarded; the upstream answers `304`. | The upstream owns the representation. |
| `HEAD` from `GET` (`headResponse`) | **Suspended.** `HEAD` is forwarded as `HEAD`. | The upstream may answer `HEAD` differently, and should not send a body to be thrown away. |
| `OPTIONS` and `405` with `Allow` (`allowUnion`) | **Suspended** for the requests an `ANY` route owns (D5): those whose path no ordinary route matches as specifically. The method is forwarded. Every other path keeps them. | The route set doesn't describe the upstream's methods. |
| CORS (`applyCors`, the preflight) | **Suspended.** `Access-Control-*` from the upstream pass; a preflight is forwarded. See D10. | Two CORS policies on one response can't both be right. |
| Status | **Any** status `100`–`599` from the upstream (`101` refused, §7.4). | The point of pass-through. |
| Error shapes, problem details, `on rejected` | **Not applied** to the upstream's body. **Kept** for the router's own rejections on the route (`400 MalformedPath`, `401`/`403` from the actor, `413`, `429`, `501`) and for `Respond(…)`. | The upstream's `404` page is the page. Bynk's own refusals stay Bynk's. |
| `Content-Length` and compression | **Passed through.** Bynk never buffers or re-encodes the body; the platform sets framing. On Workers, a `Content-Encoding` body is decoded on read and re-encoded on send (live check S2). | Bynk owns neither. |
| `Set-Cookie` | **Stripped by default**; with `@responseCookies(true)`, kept with `Domain` removed and `Path` re-scoped under the mount (§7.3). | An upstream cookie must not land on the proxy's origin unasked (§8). |
| `Location` | **Rewritten** when it points into the upstream origin (§7.5). | Keeps the browser on the proxy. |
| `X-Content-Type-Options: nosniff`, HSTS | **Kept**, from the service's `security { }`. | They protect the proxy's origin, whose policy they are. |
| HTML headers (`frameAncestors`, `referrerPolicy`, `csp`) | **Filled, not overwritten:** stamped on a `text/html` response only where the upstream sent none. | The proxied site knows its own CSP; a missing one still gets Bynk's safe default. |
| `Cache-Control`, `Vary`, `Expires`, `Age` | **Passed through**, except: forced `private` (unless `no-store`) when the route's actor carries a credential, when `Authorization` or `Cookie` was forwarded, or when a `Set-Cookie` survives. | Keeps one caller's answer out of shared caches (§8). |
| `@cache` | **Refused** on an `ANY` route (`bynk.http.cache_on_pass_through`). | The upstream's `Cache-Control` is the freshness. |
| `EdgeCache` (`edgeCacheGuard`) | **Kept, and tightened:** an answer with no `Cache-Control` gets `no-store`, as today; so does an answer whose `Vary` names anything other than `Accept-Encoding`. Otherwise its `Cache-Control` passes, after the `private` rule above. | Cloudflare's cache keys on the URL and ignores `Vary` except `Accept-Encoding`, so an answer that varies on `Accept-Language` or `Cookie` would be served to everyone (live check S4). |
| Hop-by-hop response headers | **Stripped** (§7.3). | RFC 9110 §7.6.1. |
| Trace | **Kept** for logs: the route's log lines carry the trace as any route's do. | ADR 1024. |

### 6.4 CORS and pass-through in one service

**[DECISION D10] A service that declares an `ANY` route may not declare
`cors { }`** (`bynk.http.cors_with_pass_through`). Put the proxy in its own
`from http` service. The CORS preflight branch excludes only the paths an
`ANY` route owns (D5), wherever it is declared. So a CORS service's
preflight never answers for a proxied path, and a catch-all proxy never
takes a preflight from a path a CORS service's routes match.
Rejected: **applying the service's CORS to `Respond(…)` answers only**, which
would give one route two CORS policies depending on the branch taken;
**letting Bynk's CORS override the upstream's**, which breaks an upstream
that serves its own API. The `SignedCookie` CSRF check still reads the
`cors` origins of the route's own service, so a proxy that needs a cookie
actor and cross-origin callers is a design this track doesn't support; none
exists.

## 7. Forwarding the request

### 7.1 The inbound request

**[DECISION D11] `request: InboundRequest`, a boundary-bound view, only on
an `ANY` route.**

| Method | Returns |
|---|---|
| `method()` | `String`, upper case |
| `headers()` | `RequestHeaders`, the same view a `headers:` parameter gets, consumed credential hidden |
| `query()` | `String`, the raw query string without `?`, `""` when there is none |

The body isn't readable: only `Upstream.forward` consumes it, once. Like
`RequestHeaders`, it can't be a field, cross a boundary or be stored
(`bynk.types.inbound_request_at_boundary`), and a parameter of this type on
any other route is `bynk.http.inbound_request_not_pass_through`. A second
`forward` with the same request faults, as a second read of a `Stream`
does.

**[DECISION D12] Header enumeration: `RequestHeaders.entries() ->
Map[String, String]`, lower-case names, repeated fields combined.**
Slice 2, on every `RequestHeaders`, not only a proxy's. This answers
#3752's Q6 for requests: a request header sent twice is combined with `", "`
(RFC 9110 §5.3) and `Cookie` with `"; "`, which is lossless for every
request header, so no multi-value form is needed. On the response side
`Forwarded` keeps the platform's own `Headers` (`getSetCookie` included), so
the lossy `Map` of `StreamResponse` never touches pass-through. A
multi-value header form for `Fetch` is left out of this track.
Rejected: `Map[String, List[String]]` (matches `result.headers()` in tests,
but a request has no field that needs it, and every caller would unwrap a
one-item list); `names()` plus `get` (two calls for one need).

### 7.2 The upstream: a declared origin

**[DECISION D13] An `Upstream` capability instance, whose origin is a
literal or a `String` constant, never a value computed at run time.**
**Maintainer ruling needed** on placing it in the portable `bynk` surface
(recommended: it needs only `fetch`, so it works under `bynk dev` and Node)
rather than `bynk.cloudflare`.

```bynk,ignore
consumes bynk { Upstream }

const BlogOrigin: String = env(BLOG_ORIGIN)

capability Pages = Upstream @origin(BlogOrigin) @timeout(30.seconds)
```

- `@origin` is required. It's read by the reader in
  `bynk-check/src/cors_origin.rs` (#4504): a serialised origin,
  `scheme://host[:port]`, lower case, no path, query, fragment, user
  information or trailing slash. The scheme is `https`, or `http` for a
  loopback host only (`localhost`, `127.0.0.1`, `[::1]`), which the system
  tier needs (§10.2). An `env(…)` constant is checked when the build
  resolves it, as #4504 does for `cors`.
- `forward(request: InboundRequest, path: String) ->
  Effect[Result[Forwarded, UpstreamError]]`. `path` must start with `/`.
  The URL is built **textually**, origin then path then `?` and the raw
  query, never by resolving a reference against a base, and the runtime
  checks the built URL's origin equals the declared one before sending. A
  `path` that doesn't start with `/`, holds `?`, `#` or a control character,
  or has a `.` or `..` segment (encoded or not) is `Err(InvalidPath)`, with
  nothing sent.
- `UpstreamError` is extensible: `Timeout`, `Unreachable`, `InvalidPath`.
- Further instance annotations, all optional: `@timeout(d)` (the head; default
  30 seconds; the body streams after), `@forwardHeaders(["authorization",
  "cookie"])` (§7.3), `@responseCookies(true)` (§7.3),
  `@forwardedFor(false)` (§7.3), `@propagateTrace(true)` (§9.3).
- `Upstream` is experimental (`[unstable] allow`), as `Fetch.sendStream` is.

Why this closes SSRF: the only host a pass-through route can reach is one
written in source or the manifest, and textual concatenation onto an origin
plus the origin check means no `path` can change the host:
`https://upstream.example` + `//other.example/x` is a path on
`upstream.example`, and `@other.example/x` is refused for not starting
with `/`.

Rejected: **`Fetch.receiveStream` plus a `Forward(StreamResponse)`**, because
its URL is any `String`, so the guarantee would rest on the author;
**an origin allowlist checked at run time**, which is a weaker form of the
same thing with more to configure; **one instance per path prefix with a
mount** (`@mount("/blog")`), which duplicates the route.

### 7.3 Header policy

**[DECISION D14] Forward every end-to-end header except a deny list; strip
credentials unless the instance opts in; replace the forwarding headers.**
**Maintainer ruling needed** on deny-list versus allow-list.

Request, from the client to the upstream:

| Header | Policy |
|---|---|
| Hop-by-hop: `connection`, `keep-alive`, `proxy-connection`, `te`, `trailer`, `transfer-encoding`, `upgrade`, and every name listed in `Connection` | Always stripped (RFC 9110 §7.6.1). |
| Platform-owned: `host`, `content-length`, `expect` | Never copied. The platform sets `Host` to the upstream's host and frames the body. The original `Host` goes in `X-Forwarded-Host`. |
| `proxy-authorization` | Always stripped. |
| `cf-*` (`cf-connecting-ip`, `cf-ipcountry`, `cf-ray`, `cf-visitor`, `cf-worker`, `cf-access-jwt-assertion`, …) | Always stripped. They describe the visitor to *this* Worker; `cf-access-*` is a credential. |
| `x-forwarded-*`, `forwarded`, `x-real-ip` | Always stripped, then replaced below. A client can send any value. |
| `traceparent`, `tracestate` | Always stripped (ADR 1024: a client's trace is never believed). See §9.3. |
| The route actor's consumed credential | Already hidden from `InboundRequest` (ADR 0899); never forwarded, even if named in `@forwardHeaders` (`bynk.http.forward_consumed_credential`). |
| `authorization` | **Stripped unless** `@forwardHeaders(["authorization"])`. |
| `cookie` | **Stripped unless** `@forwardHeaders(["cookie"])`. A `SignedCookie` route's session cookie stays hidden even then. |
| Every other header (`accept*`, `if-*`, `range`, `content-type`, `user-agent`, `origin`, `referer`, `sec-fetch-*`, client hints, custom headers) | Forwarded as received. |
| Added: `x-forwarded-for` | The client's address from `CF-Connecting-IP`, unless `@forwardedFor(false)`. |
| Added: `x-forwarded-proto`, `x-forwarded-host` | The scheme and `Host` the client used. |

`@forwardHeaders` accepts only `authorization` and `cookie`; any other name
is `bynk.capability.invalid_option`, since everything else is already
forwarded or never may be.

Response, from the upstream to the client:

| Header | Policy |
|---|---|
| Hop-by-hop, `proxy-authenticate` | Always stripped. |
| `set-cookie` | **Stripped unless** `@responseCookies(true)`. Then each cookie's `Domain` is removed (it becomes host-only on the proxy) and its `Path` is placed under the mount (`/` → `/blog/`, `/x` → `/blog/x`). A `__Host-` cookie, which must have `Path=/`, is dropped and logged. |
| `location` | Rewritten per §7.5. |
| Every other header | Passed through, subject to §6.3's `Cache-Control` rule and security-header filling. |

`X-Forwarded-For` sends the visitor's IP address to the upstream. For an
upstream the author runs, that's the point; for a third party, it's
personal data leaving the service. The default follows the brief
(`X-Forwarded-*` added), with `@forwardedFor(false)` to turn it off;
**Maintainer ruling needed** on whether the default should be off instead.

Rejected: **an allow list** of headers to forward (`accept*`, `if-*`,
`range` …). Safer on paper, but the set of benign end-to-end headers is
open (client hints, `sec-fetch-*`, new `accept-*` forms), and a proxy that
drops one silently breaks content negotiation or caching, which is hard to
diagnose. The headers that carry risk are few and named above.
**Forwarding cookies by default** (the TypeScript proxy does): the proxy's
own cookies are for the proxy's origin, and sending them to another origin
is the leak §8 lists.

### 7.4 Method and body

**[DECISION D15] The method passes through; the body streams under the
route's cap.**

- `GET`, `HEAD`, `POST`, `PUT`, `PATCH`, `DELETE` and `OPTIONS` are sent as
  received. `GET` and `HEAD` carry no body.
- The body is read with `requestBodyStream` under the route's effective
  `maxBody` (default 1 MiB, ADR 0165; `@limit` raises it) and streamed to
  the upstream as `Fetch.sendStream` streams. A declared `Content-Length`
  over the cap is `413` before anything is sent; a body that runs past it
  aborts the upstream request and answers `413` (the `PayloadTooLarge`
  rejection, so `on rejected` renders it). The handler never sees an `Err`
  for it.
- When the request declared a `Content-Length` within the cap, the upstream
  gets the same length (on Workers, a `FixedLengthStream`), so an endpoint
  that refuses chunked uploads still works (live check S5). Otherwise the
  body is sent chunked.
- An `Upgrade` request (WebSocket) is answered `501`, not forwarded. A `101`
  from the upstream can't occur without one.
- Redirects are never followed (§7.5).

### 7.5 Upstream redirects

**[DECISION D16] Redirects are returned, not followed; a `Location` into the
upstream origin is rewritten onto the proxy; any other passes unchanged.**
**Maintainer ruling needed** on passing off-origin redirects through.

- `forward` always sends with `redirect: "manual"`.
- A `Location` is resolved against the URL that was sent. If its origin is
  the upstream's, it's rewritten by reversing the route's own mapping:
  - **The mount `M`** is rebuilt canonically from the segments the route
    matched before its rest: `/` followed by those segments, each as it
    appeared in the request (still percent-encoded), joined by single `/`.
    It is never sliced from the raw request path. The router drops empty
    segments (§2.1), so a request for `//blog/posts/x` still matches
    `/blog/*path` with `R` = `/posts/x`. Slicing would give `M` = `//blog`,
    and a rewritten `Location` of `//blog/posts/x/` would be a
    protocol-relative URL for the host `blog`. Under `ANY("/:tenant/*path")`,
    a request for `//evil.example/x` would let the requester choose that
    host. Rebuilt, `M` always starts with exactly one `/`: `/blog`, or
    `/evil.example` as a path on the proxy.
  - **The upstream base `B`.** `forward` knows the upstream `path` `P` it
    sent, and the router knows the raw rest `R` it bound. When `P` ends with
    `R`, `B` = `P` without `R`, with any trailing `/` removed.
  - **Matching is on a segment boundary.** A `Location` path is inside `B`
    when it equals `B`, or starts with `B` + `/`. A plain string-prefix
    test would treat `/v1beta/x` as inside `B` = `/v1` and produce
    `/blogbeta/x`. An empty `B` contains every path.
  - **The rewrite** is `M` + the `Location` path with `B` removed, plus its
    query and fragment. For the blog, `M` = `/blog`, `B` = `""`, and
    `/posts/x/` becomes `/blog/posts/x/`, as the TypeScript proxy does. A
    `"/v1" + rest` mapping also reverses (`B` = `/v1`; `/v1/x` becomes
    `/blog/x`).
  - **A last check:** a rewritten `Location` that doesn't start with exactly
    one `/` is not sent. The resolved absolute `Location` is sent instead.
- When the mapping can't be reversed (`P` doesn't end with `R`, or the
  `Location` is outside `B`), the resolved absolute `Location` is passed
  through. It still points at the declared upstream, never elsewhere.
- **The same `M`** places an opted-in `Set-Cookie`'s `Path` (§7.3), so a
  cookie can't be scoped by a path the requester chose with extra slashes.
- A `Location` off the upstream's origin passes through unchanged, as the
  TypeScript proxy does ("leaves a redirect to another site alone"). The
  upstream is trusted to serve content on the proxy's origin, which is
  strictly more power than a redirect; refusing its off-site redirects
  would buy little and break login flows. The threat (an upstream open
  redirect becoming one on the proxy's origin) is in §8.

Rejected: following redirects (the browser would see the final page under
the original URL, and relative links would break); refusing off-origin
redirects by default (the blog links to GitHub through a redirect in its
tests).

## 8. Threat model

**Assets:** the proxy origin's cookies and credentials; visitors' sessions
and addresses; the integrity of what the proxy origin serves; the upstream's
credentials; the shared caches between.

**Adversaries:** a public client (any request); a cross-site page in a
visitor's browser; a compromised or misbehaving upstream; another user of a
shared cache.

| Attack | Mitigation | Where |
|---|---|---|
| **SSRF**: a request steers the proxy to another host (protocol-relative, backslash or user-information tricks in the path) | The origin is declared (`@origin`, literal or constant); the URL is built textually and its origin checked before sending; `path` must start with `/`. No request data can reach the host part. | D13 |
| SSRF into the upstream's own private paths via `..` | The rest can't hold a dot segment (WHATWG parsing, plus the router's check); `forward` refuses a dot segment in `path`. | D4, D13 |
| SSRF to internal addresses | `global_fetch_strictly_public` already; plus loopback only for an `http` origin. | §2.5, D13 |
| **Credential leakage to the upstream** | `Authorization` and `Cookie` stripped unless named; the actor's consumed credential never forwarded; `cf-access-*` and `proxy-authorization` always stripped. | D14 |
| **Credential leakage from the upstream** | `Set-Cookie` stripped unless asked; `proxy-authenticate` stripped. An upstream `401` with `WWW-Authenticate: Basic` reaches the browser, whose answer is then stripped as `Authorization` by default: no loop, no leak. | D14 |
| Visitor IP leaked to a third party | `X-Forwarded-For` can be turned off. | D14 |
| **Cache poisoning**: an upstream answer varying on a header the cache doesn't key on | The edge cache doesn't honour `Vary` beyond `Accept-Encoding`, so it isn't counted as a control: under `EdgeCache`, an answer whose `Vary` names anything else is `no-store`, and only answers the upstream marked cacheable are stored at all. `Vary` still passes through for browsers and other caches downstream. | D9 |
| One user's answer stored in a shared cache | `Cache-Control` forced `private` when the actor carries a credential, when `Authorization` or `Cookie` was forwarded, or when a `Set-Cookie` survives. | D9 |
| Poisoning through forwarding headers (`X-Forwarded-Host` used by the upstream to build links, cached) | Client-sent `x-forwarded-*`, `forwarded` and `x-real-ip` are always replaced, so the upstream sees only what the proxy saw. | D14 |
| **`Set-Cookie` scoping**: an upstream sets `Domain=bynk-lang.org` or `Path=/` and reaches the main site's cookies | Stripped by default; when opted in, `Domain` removed and `Path` placed under the mount; `__Host-` dropped. | D14 |
| **Open redirect** via the upstream | `Location` into the upstream is rewritten onto the proxy; off-origin redirects pass, documented, with the reasoning in D16 (the upstream can already serve arbitrary content on the origin). A guide note: proxy only an origin you would host. | D16 |
| Open redirect through the **rewrite itself**: extra slashes in the request (`//blog/…`), or a parameter in the mount, make the rewritten `Location` protocol-relative | The mount is rebuilt from the matched segments, never sliced from the raw path; the base matches on segment boundaries; a rewritten `Location` must start with exactly one `/`. | D16 |
| An upstream serving script on the proxy's origin | Out of Bynk's reach by design: the upstream *is* content on that origin. `nosniff` and the HTML security headers filled where absent narrow the damage. | D9 |
| **Request smuggling** between proxy and upstream | Bynk never writes HTTP framing: `Transfer-Encoding`, `Content-Length`, `Connection` and `Expect` are never copied, header values go through the platform's `Headers` (which refuses line breaks), and the platform frames the body. | D14, D15 |
| **Oversized request bodies** | The route's `maxBody` (default 1 MiB), counted as the body streams; `413` before or during. | D15 |
| Oversized or slow upstream responses | Streamed, never buffered, so size can't exhaust memory; `@timeout` bounds the head (`504`); a body that stalls is bounded by the platform's limits. | D13, D15 |
| `TRACE` reflecting credentials; method confusion | Only seven methods forwarded; others `501`. | D7 |
| WebSocket upgrade tunnelling | `Upgrade` refused with `501`. | D15 |
| Login CSRF / CSRF through a proxied state-changing route | §9.2: the `SignedCookie` check applies; on `by Visitor` with no forwarded cookie there is no ambient credential to abuse. | §9.2 |

## 9. Composition

### 9.1 Actors

**[DECISION D17] A pass-through route takes `by`, like any route.**
The actor's seam runs before the handler and before anything is forwarded;
a `401` or `403` is the router's own, in Bynk's shape. The consumed
credential is hidden from `InboundRequest` and never forwarded (D14). A
credentialed actor forces `Cache-Control: private` (D9). `key: identity`
rate limiting works as on any route.
Rejected: refusing `by` on `ANY` routes (an authenticated proxy in front of
an internal dashboard is a common, legitimate use).

### 9.2 CSRF and `Origin` rules (#3825, #4504)

- A `SignedCookie`-guarded pass-through route runs the actor's CSRF check
  (ADR 0821: fetch metadata, then `Origin` against the service's `cors`
  origins) on every method except `GET`, `HEAD` and `OPTIONS`, since `ANY`
  has no fixed method list. Under D10 such a service has no `cors` block,
  so only same-origin requests pass the check.
- #3825 (accepted, not built) marks handlers that call `Actor.issue`. A
  pass-through handler can call it in a `Respond(…)` branch and is marked
  the same way; nothing pass-through-specific is needed.
- #4504's origin reader (`cors_origin.rs`) validates `@origin` (D13), so
  the two rules share one definition of "an origin".

### 9.3 Other features

- **`EdgeCache`**: D9's row. A `public` upstream answer on a route
  `by Visitor` with no credentials forwarded is cached at the edge by URL;
  anything credentialed is `private`.
- **Rate limiting**: `@rateLimit` works unchanged, asked before anything is
  forwarded.
- **Tracing** — **[DECISION D18] don't propagate `traceparent` by default;
  `@propagateTrace(true)` sends the current trace (new span id) to an
  upstream the author runs.** The client's own `traceparent` is always
  stripped (ADR 1024). Rejected: always propagating (a correlation id
  handed to a third party, for no benefit to one that doesn't trace);
  never (an author's own upstream behind the proxy is exactly where a
  shared trace helps).
- **Timeouts**: `@timeout` bounds the response head (default 30 seconds),
  as `sendStream`'s does; the body streams after. A Worker's own wall-clock
  limits still apply.
- **Upstream failure** — **[DECISION D19] `forward` returns
  `Err(Timeout)` or `Err(Unreachable)`; the handler maps them, and the guide
  and the blog map `Timeout` to `504 GatewayTimeout` and everything else to
  `502 BadGateway`.** A failure after the head arrived cuts the response
  short, as `RawStreamed` does. Rejected: the router mapping failures itself
  (the handler then can't serve a fallback page or log what it wants).

## 10. Testing

### 10.1 Unit tier

**[DECISION D20] The `Upstream` double has two seams: `forward`, for the
handler's own logic, and `exchange`, beneath the forwarding policy.**

`forward(request, path)` is the policy (§7.3, §7.5) followed by one
`exchange(sent: UpstreamRequest) -> Effect[Result[Forwarded, UpstreamError]]`,
the network step. `exchange` is a seam only in suites: production code can't
call it. `UpstreamRequest` is a test-visible record of what the policy built:
`method: String`, `url: String`, `headers: Map[String, String]` and
`hasBody: Bool`.

- **Stub `exchange` to test the policy.**
  `stub Pages.exchange(_) returns Ok(Forwarded.of(308, ["location: /posts/x/"], Stream.of([])))`
  sets what the upstream answers, and the real `forward` runs around it, so
  header stripping and the `Location` rewrite both happen. What the policy
  sent is observed on the same seam:
  `expect Pages.exchange called once with sent.headers.get("cookie") == None`
  and `sent.url == "https://upstream.example/posts/x"`. This is how every
  row of §7.3 and §7.5 gets a unit-tier case.
- **Stub `forward` to test only the handler.**
  `stub Pages.forward(_, _) returns Err(Timeout)` replaces the whole method,
  policy included, for a case about the handler's own branches (the
  `504` mapping, the `/blog` redirect). Such a case says nothing about the
  policy.
- `Forwarded.of(status, headerLines, body)` exists only in suites, so
  production code still can't fabricate one.
- An `ANY` route is addressed by the method sent, then the pattern and its
  parameters: `proxy.GET("/blog/*path", "/posts/x")`, and new `HEAD` and
  `OPTIONS` addresses valid only on `ANY` routes. A trailing
  `RequestHeaders` sets the inbound headers. The outcome is a
  `PassThrough`.
- The runtime's policy functions get TypeScript unit tests of their own, as
  `runtime/http.ts` has today.

Rejected: stubbing `Fetch.receiveStream` (the brief's first thought). The
forwarding policy sits above the exchange, so a `Fetch` stub would test the
handler but bypass the policy; and pass-through no longer goes through
`Fetch`'s `Map` headers (D12).

### 10.2 System tier

A workerd case against a local upstream, on the precedent of `upstream()`
and `fetch_send_with_times_out_and_returns_a_manual_redirect_on_workerd` in
`bynk-tests/tests/system/binding_workerd.rs`: a `127.0.0.1` listener on a
free port, its origin given to the project through `env(…)` in the case's
`bynk.toml`. It exercises, end to end: status pass-through (`200`, `304`,
`404`, `308`), `HEAD` and `OPTIONS` forwarded, streamed bodies both ways,
the header policy seen by the upstream (cookies stripped, `X-Forwarded-*`
replaced), `Location` rewriting, `Set-Cookie` stripping and re-scoping,
`413` on an oversized body, `504` on a stalled head, and `/_bynk/` and a
CORS preflight never reaching an `ANY` route. A root catch-all
`ANY("/*path")` in the same Worker as a CORS service: that service's
paths keep their preflight, `405` with `Allow` and `HEAD`, and none of
their requests reach the upstream.

### 10.3 Live only: sandbox scripts

Lanes never touch accounts, so anything that needs Cloudflare's real edge is
a script for the maintainer to run against a sandbox account, kept beside
the slice that needs it and named in its "Done when":

- **S1.** Whether the edge passes `//`, `/\`, `%2e%2e` and `%2F` paths to a
  Worker unchanged, under the zone's default URL normalisation. Confirms
  D4's assumptions.
- **S2.** A brotli- and gzip-encoded upstream (Pages) through a pass-through
  Worker: no double encoding, a correct `Content-Encoding`, and what
  happens to `Content-Length`.
- **S3.** What Cloudflare itself sets on a subrequest to another zone:
  `CF-Connecting-IP`, `X-Forwarded-For`, and whether `cf-*` request headers
  can be set or are dropped.
- **S4.** `EdgeCache` storing a pass-through answer: `cf-cache-status`, a
  `private` answer not stored, an answer with `Vary: accept-language` not stored, and whether the Cache API honours any `Vary` at all.
- **S5.** A `PUT` with `Content-Length` arriving at the upstream with the
  same length (`FixedLengthStream`), not chunked.
- **S6.** A Worker route on a path of a zone whose host is a Pages custom
  domain still takes precedence (the blog relies on it today).
- **S7.** Several `Set-Cookie` headers end to end, when opted in.

## 11. Acceptance by dogfooding

### 11.1 The reference: every behaviour of the TypeScript proxy

From `bynk-lang/blog` `proxy/src/index.ts` (`handle`), its tests and
`proxy/wrangler.toml`:

| # | Behaviour | Test |
|---|---|---|
| B1 | `/blog` (no slash) is a `308` to `/blog/`, keeping the query; `Location` absolute (`https://bynk-lang.org/blog/?ref=x`), for any method. | "redirects /blog to /blog/, keeping the query" |
| B2 | A path outside `/blog/` (a misconfigured route) is `404` with body `Not found`. | "refuses a path outside /blog/" |
| B3 | `/blog` is stripped and the same path and query are fetched from `UPSTREAM`. | "fetches the same path…", "serves the blog's home page at /blog/" |
| B4 | The method, every header (cookies and `Authorization` included) and the body are forwarded. | "passes the method, headers and status through" |
| B5 | Redirects are not followed (`redirect: "manual"`). | "asks for redirects to come back…" |
| B6 | With no `Location`, the response is returned unchanged: status, status text, headers, body. `304`, `404` and `HEAD` included. | "passes the method…", "passes a 404…" |
| B7 | A `Location` resolving to the upstream's origin becomes `/blog` + its path, query and fragment, relative. | "puts the prefix back…", "rewrites an absolute redirect…" |
| B8 | A `Location` to another origin is unchanged. | "leaves a redirect to another site alone" |
| B9 | Deploy: script `bynk-blog-proxy`, routes `bynk-lang.org/blog` and `bynk-lang.org/blog/*` on zone `bynk-lang.org`, `workers_dev = false`, `UPSTREAM = "https://bynk-blog.pages.dev"`. | `wrangler.toml` |
| B10 | CI: typecheck, tests, dry-run deploy; deploy after CI on `main`. | `.github/workflows` |
| B11 | (Unintended) The host-confusion issue in §2.7. | none |

### 11.2 The port

```bynk,ignore
context blog

consumes bynk { Upstream }

const BlogOrigin: String = env(BLOG_ORIGIN)

capability Pages = Upstream @origin(BlogOrigin) @timeout(30.seconds)

service proxy from http {
	on ANY("/blog/*path") (path: String, request: InboundRequest)
			-> Effect[PassThrough] by Visitor given Pages {
		if path == "" {
			let q = request.query()
			Respond(PermanentRedirect(if q == "" { "/blog/" } else { "/blog/?\(q)" }))
		} else {
			let got <- Pages.forward(request, path)
			match got {
				Ok(response) => Forward(response)
				Err(Timeout) => Respond(GatewayTimeout("the blog did not answer in time"))
				Err(_) => Respond(BadGateway("the blog could not be reached"))
			}
		}
	}
}
```

One route replaces both Worker routes' logic, because `/blog/*path`
matches `/blog` with an empty rest (D4). The manifest carries B9's routes in
`[deploy.workers.<key>] routes`, and `BLOG_ORIGIN` per environment.

### 11.3 Behavioural equivalence

**[DECISION D21] "Behaviourally equivalent" means: the ten TypeScript tests,
restated as Bynk suite cases and a workerd case, pass against the Bynk
Worker, with only these intended differences, each documented in the port's
PR.** **Maintainer ruling needed** on accepting each difference.

| Behaviour | TypeScript | Bynk | Why the difference |
|---|---|---|---|
| B1 `Location` | absolute | relative `/blog/?ref=x` | Equivalent for every browser (RFC 9110 §10.2.2); a handler doesn't need the host. |
| B2 | `404 Not found` | the router's `404` (no route matches) | The Worker's routes only send `/blog*`, so unreachable in production. |
| B4 cookies, `Authorization` | forwarded | stripped | The blog uses neither; D14's default. |
| B4 forwarding headers | none added | `X-Forwarded-*` added, client values replaced | D14. |
| B6 status text | kept | not kept | HTTP/2 and HTTP/3 have none. |
| B6 `Set-Cookie` | kept | stripped | The blog sets none; D14. |
| B6 security headers | none | `nosniff`; HTML headers filled where absent | D9. The blog's pages must not be framed anyway. |
| B11 | present | ruled out | D13. |

### 11.4 Where the port lives

**[DECISION D22] The port replaces `proxy/` in `bynk-lang/blog`, which
already owns the deploy workflow and its secrets; the showcase links to it.**
**Maintainer ruling needed** — #3752's sketch put it in
`bynk-lang/showcase`. Rejected: a copy in each (two deploys of one route).

## 12. The 1.0 position

**Additive, with no Foundations behaviour changed**, under this doc's
recommendations:

- **Rest segments** use `*`, which ADR 0879 reserved for this; every route
  that compiles today means the same thing.
- **`ANY`** is a new word in the method position, a parse error today.
- **`PassThrough`, `Forwarded`, `InboundRequest`, `Upstream`,
  `UpstreamError`** are new names. New built-in variant names resolve by
  expected type, so a program's own `Forward` or `Respond` keeps compiling
  (checked, §6.2).
- **`RequestHeaders.entries()`** is a new method on an existing view.
- **`HttpResult` is untouched**, deliberately (D7, D8).
- **Router behaviour on existing routes is unchanged.** A suspension, and
  the CORS preflight exclusion, apply only to requests an `ANY` route owns
  (D5): those whose path no ordinary route matches as specifically. Adding
  a catch-all `ANY("/*path")` leaves every existing route's `405`, `Allow`,
  `HEAD` and preflight as they were.

Where it **would** change Foundations behaviour, and why the
recommendations avoid it:

- **A `Forward` variant on `HttpResult`** would break every exhaustive
  `match` on `HttpResult` (`bynk.types.non_exhaustive_match`, §2.2).
  Pre-1.0 that's a minor-release break, as `RawCreated` and
  `PartialContent` were; after 1.0, a 2.0 event. Hence D8's separate type.
- **Matching one-or-more segments with a literal `/blog` route** would need
  the empty-segment rule (#3909) changed for that case, which changes
  existing routes. Hence D4's zero-or-more.
- **Making the rest decoded** wouldn't change existing behaviour, but would
  freeze a lossy choice into the 1.0 surface.

**[DECISION D23] Separately from this track, decide before 1.0 whether
`HttpResult` should be extensible** (a `match` on it needs a wildcard), so
the RFC 9110 vocabulary (ADR 0126) can grow through 1.x. **Maintainer ruling
needed.** It's a breaking change now (every exhaustive `match` gains a
wildcard), and cheap before 1.0; this track doesn't depend on it.

All new items enter as experimental (`[unstable] allow`), so none join the
frozen surface until promoted.

## 13. Slices

Each is one reviewable PR, cut as an increment proposal on the spine, with
its own pending file, ADR and docs. Sizes are estimates.

| # | Slice | Depends on | Done when | Size |
|---|---|---|---|---|
| 1 | **Rest segments and the router.** `*name` (D2–D6) in ordinary routes: checker rules and diagnostics, `route_order` with `Seg::Rest`, `matchPath`, `param_count`, `Rest_` naming and the duplicate-name check, `bynk client` and `bynk openapi` leaving rest routes out, the test address, LSP hover. | — | A `GET /files/*key` serves R2 keys with `/` at unit and workerd tiers; every row of §5.2 and §5.4 is a fixture; `unreachable_route` covers rests; `HEAD`, `405` and CORS work on a rest route; `/_bynk/` unaffected under `/*path`; ADR and reference updated. | M |
| 2 | **Header enumeration.** `RequestHeaders.entries()` (D12), consumed credentials still hidden, `RequestHeaders.of` and the system-tier driver honouring it. | — | A handler lists a request's headers; repeated fields arrive combined; a `Bearer` route's `entries()` has no `authorization`. | S |
| 3 | **The pass-through route and response.** `ANY` in the grammar, parser, IR, tree-sitter and `vscode-bynk`; `PassThrough`, `Forwarded`, `InboundRequest`; `Upstream` with `@origin` and `@timeout`; `forward` sending method, path, raw query and body, **and no inbound headers**; every row of D9 except `Set-Cookie` re-scoping and `Location` rewriting; D10; `501` for other methods and `Upgrade`; the unit-tier double (D20). | 1 | The status, headers and streamed body of a local upstream pass through at workerd tier, `200`, `304`, `404`, `308` included; no `ETag`, `HEAD` or `OPTIONS` is synthesised on an `ANY` path; the SSRF cases of §8 are refused at unit and workerd tiers. Safe but not yet useful: an upstream gets no `If-None-Match`. | L |
| 4 | **Forwarding with the safety defaults.** The header policy (D14) with `@forwardHeaders`, `@forwardedFor`, `@responseCookies`; `Location` rewriting (D16), with fixtures for a request with extra leading slashes, a parameter in the mount, and a base that is a string prefix but not a segment prefix (`/v1` against `/v1beta`); the `Cache-Control` and `Vary` rules (D9); `@propagateTrace` (D18); the body length (D15); the workerd case of §10.2 in full; sandbox scripts S1–S7 written. | 2, 3 | Every row of §7.3 and §8 has a unit or workerd case; the scripts are in the PR, with their results recorded by the maintainer before the slice is marked done. | M |
| 5 | **The blog port.** §11.2 replacing `bynk-lang/blog` `proxy/` (or per D22), its tests restated, the deploy switched; the README's "Why not Bynk?" retired; a guide, *Serve another site under a path*. | 4 | The ten tests of §11.1 pass against the Bynk Worker with only §11.3's differences; deployed and checked at `bynk-lang.org/blog/` and a post; S1's answer recorded. | M |

The track retires after slice 5, with this doc's closing summary in
`archive/retired-tracks.md`.

**ADR candidates**, each landing with its slice rather than up front, since
no slice's direction depends on another's ADR: the rest segment (slice 1,
amending ADR 0879 and 0914); header enumeration (slice 2); the pass-through
route form and its suspensions (slice 3, relating to ADR 0162, 0163, 0159,
0164); the forwarding policy (slice 4).

## 14. Docs delta

| Page | Change | Slice |
|---|---|---|
| `reference/http.md` §Handler form, §How a route is chosen, §Path parameters | The rest segment, its binding, edge cases and precedence; "reserved for a future form" narrowed to `{`, `}` and `*` outside a last segment | 1 |
| `reference/http.md` §Request headers | `entries()` | 2 |
| `reference/http.md`: new §Pass-through routes | `ANY`, `PassThrough`, `InboundRequest`, the suspensions table | 3 |
| `reference/http.md` §Method semantics, §Caching, §CORS, §Security headers, §Request lifecycle | A sentence each on `ANY` paths, and the flowchart's new branch | 3 |
| `reference/bynk-capabilities.md`: new §`Upstream` | The instance, `forward`, `UpstreamError`, the header policy, redirects | 3, 4 |
| `reference/bynk-capabilities.md` §Streamed bodies | Point the proxy example at `Upstream`; keep `RawStreamed` for non-proxy streaming | 4 |
| `reference/testing.md` §suite blocks, §stub | The `ANY` addresses (`HEAD`, `OPTIONS`), `Forwarded.of`, observing `sent` | 1, 3 |
| `reference/http-client.md` §Routes left out | Rest and `ANY` routes | 1, 3 |
| `reference/grammar.md`, `grammar-appendix.md`, `keywords.md` | `ANY`; the rest segment in the route production | 1, 3 |
| `reference/glossary.md` | rest segment, pass-through route, upstream | 1, 3 |
| `book/guides/entry-points/`: new *Serve another site under a path* | The blog port as the worked example, with the threat notes | 5 |
| `book/guides/entry-points/`: new section or guide, *Serve files whose keys contain `/`* | R2 over a rest segment, with `percentDecode` | 1 |
| Diagnostics explain entries (`site/src/generated/diagnostics`) | Each new code | 1, 3, 4 |
| `bynk-lang/blog` `proxy/README.md` | "Why not Bynk?" retired | 5 |

## 15. Risks

- **The `ANY` surface may be wrong for other proxies.** Mitigation: slice 3
  is experimental, and §11 tests it against a real one.
- **Platform behaviour differs from the design's assumptions** (S1–S7).
  Mitigation: the scripts gate slice 4's "done", and D4's router check holds
  even if the edge passes dot segments.
- **Slice 3 is large.** If review needs it, split the grammar and types
  (`ANY`, `PassThrough`, refusing everything else) from the router
  suspensions and the `Upstream` exchange.
- **Header policy drift between `Fetch` and `Upstream`.** Both strip
  `FORBIDDEN_REQUEST_HEADERS`; slice 4 shares that list rather than copying
  it.

## 16. Decisions index

| | Decision | Ruling |
|---|---|---|
| D1 | Pass-through in the ordinary service tier, as a distinct route form | **Maintainer ruling needed** |
| D2 | `*name`, last segment only | |
| D3 | The rest binds a raw `String` | |
| D4 | Edge cases; zero-or-more matching | |
| D5 | Rest is the weakest segment; an `ANY` route owns only the paths it matches more specifically than every ordinary route | |
| D6 | Router and naming changes | |
| D7 | `on ANY(…)` with `Effect[PassThrough]` | **Maintainer ruling needed** |
| D8 | `PassThrough = Forward(Forwarded) \| Respond(HttpResult[()])` | |
| D9 | What `Forward` suspends and keeps | |
| D10 | No `cors` in a service with an `ANY` route | |
| D11 | `InboundRequest` | |
| D12 | `RequestHeaders.entries()`; no multi-value form | |
| D13 | `Upstream` with a declared origin; portable `bynk` | **Maintainer ruling needed** (placement) |
| D14 | Deny-list header policy; credentials opt-in; `X-Forwarded-*` | **Maintainer ruling needed** (deny versus allow list; `X-Forwarded-For` default) |
| D15 | Method and streamed body under the cap | |
| D16 | Manual redirects; `Location` rewrite; off-origin passes | **Maintainer ruling needed** (off-origin) |
| D17 | `by` allowed | |
| D18 | `traceparent` opt-in | |
| D19 | Failures mapped by the handler, `504` and `502` | |
| D20 | The `Upstream` double: `exchange` beneath the policy, `forward` for the handler | |
| D21 | Equivalence, and its intended differences | **Maintainer ruling needed** |
| D22 | The port lives in `bynk-lang/blog` | **Maintainer ruling needed** |
| D23 | `HttpResult` extensible before 1.0 (outside this track) | **Maintainer ruling needed** |
