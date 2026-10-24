---
level: minor
changelog: "`bynk.cloudflare` gains `EdgeCache`: declaring `capability Edge = EdgeCache` turns Cloudflare's Workers Cache on for the context's Worker (`[cache] enabled = true`), `purge(prefixes) -> Effect[Bool]` purges it by path prefix, and an opted-in Worker sends `no-store` on every `GET`/`HEAD` answer but a `@cache` route's `2xx`, so a `404` is never cached; refused from agents (`bynk.cloudflare.edge_cache_in_agent`), queue consumers and exported capabilities, at most one per context (first-party registry L7, [#2414](https://github.com/bynk-lang/bynk/issues/2414))"
---

## ADR: edge-cache
title: Purgeable edge caching as an `EdgeCache` capability
summary: Declaring an `EdgeCache` instance turns Workers Cache on for a context's Worker; it purges by path prefix through the entry's `ctx`; an opted-in Worker stores nothing but a `@cache` route's `2xx`; refused where the purge would not reach the Worker's own cache

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
- **D. The purge is used only where the Worker's own entry is.** That is
  service handlers, `on schedule`, and the providers and functions they call.
  An agent handler whose `given` reaches an instance, directly or through a
  context provider's `given` at any depth, is
  `bynk.cloudflare.edge_cache_in_agent`: an agent runs in a Durable Object,
  another entrypoint. A queue consumer is refused too
  (`bynk.cloudflare.edge_cache_in_queue`). Cloudflare's docs say queue consumers
  run without cache involvement but don't say whether their purge reaches the
  fetch entrypoint's cache, so the refusal stands until that is confirmed. A
  derived capability can't be exported, but a capability whose provider is
  given one can. A consuming context builds that provider in its own compose,
  where `exec` may not exist (a `ReferenceError` on every request) and where a
  purge would reach the consumer's cache, not the owner's. So an exported
  capability that reaches an instance is refused at its declaration
  (`bynk.cloudflare.edge_cache_exported`). Another context purges by calling a
  service of the owning context.
- **E. The entry's `ctx` reaches the binding.** `WorkersEdgeCache(ctx)` is
  constructed in `compose` as `new WorkersEdgeCache(exec)`. `compose` takes
  `exec` whenever the context has an instance (optional when nothing sends, as
  the entry's own `ctx` is), and `scheduled` takes `ctx` too. No other
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
    it.
  - Purges apply only to the calling entrypoint and resolve
    `{ success, errors }`.
- **Deployed findings:** a scratch opted-in Worker, deployed once credentials
  are available, records here whether a hit skips the Worker, whether
  `Authorization` bypasses, how fast a prefix purge lands, the Free-tier purge
  limit, and whether a `scheduled` purge reaches the fetch cache.

**Consequences.** The registry service can mark its archives
`@cache(immutable: true, scope: public)` in a context with `EdgeCache`, purge
on takedown and re-run a takedown whose purge was refused (R6.1, #2413). A
service that adds the capability changes its `GET` headers, and the changelog
and the guide say so. `scope: public` in an opted-in context means shared at
Cloudflare's edge. Nothing joins `bynk.deploy.lock`, because Workers Cache has
no namespace to allocate.
