# 0613 — Purgeable edge caching as an `EdgeCache` capability

- **Status:** Accepted (v0.404)

**Context.** L3's `@cache(maxAge:, scope:, immutable:)` (#2335) only stamps a
`Cache-Control` header. Cloudflare doesn't store a response a Worker generates
because of that header, so nothing a Bynk Worker served was cached at the edge.
A public registry needs its archives cached there. A takedown deletes an
archive (first-party registry D8, R5a.2), so whatever caches it must be
purgeable. `applyCache` also stamped the route's header on whatever the handler
returned, a `NotFound` included. Under RFC 9111 a `404` is heuristically
cacheable, so on `/dl/sha256/:hex` a passing miss would be kept as `immutable`
for a year. Cloudflare's Workers Cache supplies the platform half. `[cache]
enabled = true` in `wrangler.toml` serves `GET`/`HEAD` from the edge before the
Worker runs. `ctx.cache.purge({ pathPrefixes })` purges globally, scoped to the
calling entrypoint and rate-limited at the Free tier's limits on every plan.
Bynk owns the generated `wrangler.toml`, and a binding sees only `env`, never
`ctx`, so an author could reach neither. This is platform work (ADR 0023); the
standard-library track's Cache-API row is a different feature (`caches.default`,
per data centre).

**Decision.**

- **A. The opt-in is declaring an `EdgeCache` instance.** `EdgeCache` is an
  `Instantiable` row of `bynk.cloudflare` with no options and `bare: false`.
  `capability Edge = EdgeCache` turns the cache on for its context's Worker,
  which gets `[cache] enabled = true`. A context without one emits a
  byte-identical `wrangler.toml`. Workers Cache is a Worker-wide setting, so a
  second instance is `bynk.cloudflare.edge_cache_duplicate`. The instance has
  no `env` binding: it is filtered out of the Worker's bindings, so it adds no
  `Env` member and can't clash in `bynk.context.duplicate_binding`.
  Experimental, behind `[unstable] allow = ["bynk.cloudflare.EdgeCache"]`.
  Rejected: caching wherever `@cache(scope: public)` appears, which silently
  changes every existing program on upgrade; a `[workers]` manifest key, which
  is project-wide while Workers are per context, and puts the switch out of
  reach of the code that must purge.
- **B. One operation, `purge(prefixes: List[String]) -> Effect[Bool]`.** It
  gives `success` from `ctx.cache.purge({ pathPrefixes })`: `true` when
  Cloudflare accepted the purge, `false` when it refused one, as
  `RateLimit.allow` reports. An empty list purges nothing and gives `true`.
  Each prefix must start with `/` and hold no `?`, `#` or control character;
  anything else faults as a programming error. Cloudflare takes paths, never a
  query or fragment. A content-addressed path has a fixed length, so its prefix
  purge is exact. Tags (which need a `Cache-Tag` response surface) and
  `purgeEverything` are deferred until a consumer needs them.
- **C. An opted-in Worker sends `no-store` on every `GET`/`HEAD` response that
  isn't a `2xx` from a `@cache` route.** The route composition uses
  `applyEdgeCache`, which is `applyCache` on a `2xx` and `no-store` otherwise. A
  `GET` without `@cache` is wrapped in `noStore`. The dispatch moves into a
  module-level `__bynkFetch`, and `fetch` runs whatever it answers through
  `edgeCacheGuard`. That marks any `GET`/`HEAD` response still without a
  `Cache-Control`: the router's `404`/`405`, a rejection, a fault's `500`. A
  `101` upgrade passes through, and a response with immutable headers is
  copied. A `304` copies the `200` it stands for, as before. Other methods are
  unchanged. Only opted-in Workers change, so no golden outside the new fixture
  moves. Rejected: a short-lived `404`, which makes a seed race visible;
  leaving it to authors.
- **D. The purge is used only from the Worker's own `fetch`.** That is
  service handlers and the providers and functions they call.
  An agent handler whose `given` reaches an instance, directly or through a
  context provider's `given` at any depth, is
  `bynk.cloudflare.edge_cache_in_agent`: an agent runs in a Durable Object,
  another entrypoint. A queue consumer is refused too
  (`bynk.cloudflare.edge_cache_in_queue`). Cloudflare's docs say queue consumers
  run without cache involvement but don't say whether their purge reaches the
  fetch entrypoint's cache, so the refusal stands until that is confirmed.
  The accepted proposal allowed `on schedule`, but the scratch deployment
  showed its purge answers `success: true` and clears nothing from the fetch
  cache (findings below). So it is refused too
  (`bynk.cloudflare.edge_cache_in_schedule`), since a purge that silently does
  nothing is worse than none. A
  derived capability can't be exported, but a capability whose provider is
  given one can. A consuming context builds that provider in its own compose,
  where `exec` may not exist (a `ReferenceError` on every request) and where a
  purge would reach the consumer's cache, not the owner's. So an exported
  capability that reaches an instance is refused at its declaration
  (`bynk.cloudflare.edge_cache_exported`). Another context purges by calling a
  service of the owning context.
- **G. In an opted-in context, a `@cache(scope: public)` route answers every
  caller alike.** The edge keys a response by its URL. Cloudflare's docs say a
  request carrying `Authorization` bypasses the cache, but `public` overrides
  that (RFC 9111 §3.5): deployed, a `public` response to an authorized request
  was stored and served to an anonymous one. A route can also authenticate
  without `Authorization` (a `Signature` header, say). So a public `@cache`
  route whose `by` names any actor that
  authenticates is refused (`bynk.cloudflare.edge_cache_public_authenticated`);
  only open actors (`Visitor`, or an actor with no scheme) may serve one.
  `scope: private` is never shared there, so it stays allowed. A Worker whose
  `Locale` negotiates from `Accept-Language` adds `Vary: accept-language` to
  its `@cache` routes, and the edge then keeps one response per language.
- **E. The entry's `ctx` reaches the binding.** `WorkersEdgeCache(ctx)` is
  constructed in `compose` as `new WorkersEdgeCache(exec)`. `compose` takes
  `exec` whenever the context has an instance (optional when nothing sends, as
  the entry's own `ctx` is). `scheduled` and `queue` pass their `ctx` too,
  since they share `compose`, though nothing there may reach the instance
  (D). No other
  binding's constructor changes. With no `ctx.cache`, the binding logs
  `bynk: EdgeCache.purge is a no-op under bynk dev` and gives `true`: under
  `bynk dev`, in the bundle, and for an in-process caller with no `ctx`. The
  tests have one double per tier. At `unit`, each instance's double accepts
  every purge, so `stub Edge.purge(_) returns false` refuses one and
  observation sees the prefixes. At `system`, the harness's `ExecutionContext`
  double (`__bynkExec`) gains a `cache` that records purges and answers
  `success: true`, settable to `false`, and an opted-in participant is always
  driven with it.
- **F. Local development is honest, not simulated.** Under `wrangler dev`
  4.149 (Miniflare) a Worker with `[cache] enabled = true` runs on every
  request, and `ctx.cache` is `undefined`. So `bynk dev` serves uncached, with
  the same headers, and `purge` is the logged no-op of E. Cached behaviour is
  proved on a deployment.

**Platform findings.**

- **Wrangler:** `[cache]` is in Wrangler's config schema from 4.107.0, which
  is `WRANGLER_MIN`, so `bynk doctor` needs no newer Wrangler.
  `wrangler deploy --dry-run` on 4.149.0 accepts the emitted stanza with no
  unknown-field warning, while a misspelt table is warned about.
- **Cloudflare's docs** (read 2026-10-24):
  - Only `fetch` invocations use the cache.
  - Durable Objects, queue consumers and cron runs always run without it.
  - A request with `Authorization`, or a response with `Set-Cookie`, bypasses
    it. A deployment shows this needs qualifying, below.
  - Purges apply only to the calling entrypoint and resolve
    `{ success, errors }`.
- **Deployed** (2026-10-24, a scratch account, wrangler 4.149.0, compatibility
  date 2026-07-01). There were two Workers, both deleted afterwards: a plain
  probe, and this slice's own emitted `2414_edge_cache`.
  - **A hit skips the Worker.** The second `GET` of a `public, max-age` response
    was `cf-cache-status: HIT` with the first run's body. On a hit the edge
    answers a matching `If-None-Match` with a `304` itself.
  - **A `404` with no `Cache-Control` is cached** (`HIT` on the second
    request), which is why Decision C is needed. `no-store` is `BYPASS`, and so
    is `private, max-age`.
  - **`Authorization` bypasses only responses that aren't `public`.** With
    `max-age` or `private` alone, an authorized request was `BYPASS`, and it
    stored nothing. With `public` (or `s-maxage`), the authorized request's
    response was stored and served to a later anonymous request. A cookie
    doesn't bypass at all. This is Decision G's evidence.
  - **A prefix purge from `fetch` lands at once.** The call took 0.2 to 0.8 s,
    and the next request was a `MISS`. A path that merely shares the prefix is
    purged too (`/a` purges `/ab`). A path outside it stays a `HIT`, so a
    fixed-length content address is exact.
  - **Rate limit:** in a burst, 25 purges were accepted, then the rest
    answered `success: false` with error `1134` ("rate limit reached"). The
    call never threw, which confirms Decision B's `Bool`. Budget was back within
    a minute.
  - **A `scheduled` purge doesn't reach the fetch cache.** A per-minute cron
    purging `/s/` logged `success: true` on every run, yet a cached `/s/…`
    entry stayed a `HIT` for over three minutes. The same purge called from
    `fetch` cleared it at once. Hence D's refusal of `on schedule`.
  - **End to end, the emitted Worker behaves as Decision C says.** The archive
    route was `MISS` then `HIT` with its `immutable` header, and its `404`, the
    `GET` without `@cache` and an unrouted path were all `no-store`/`BYPASS`.
    A takedown `POST` answered `true`, and the archive's next fetch was a
    `MISS`.
  - **Not probed:** a queue consumer's purge (still refused) and a Durable
    Object's (refused, and the docs say DOs never use the cache).

**Consequences.** The registry service can mark its archives
`@cache(immutable: true, scope: public)` in a context with `EdgeCache`, purge
on takedown and re-run a takedown whose purge was refused (R6.1, #2413). A
service that adds the capability changes its `GET` headers, and the changelog
and the guide say so. `scope: public` in an opted-in context means shared at
Cloudflare's edge. Nothing joins `bynk.deploy.lock`, because Workers Cache has
no namespace to allocate.
