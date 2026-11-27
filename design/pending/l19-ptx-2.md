---
level: minor
probes: emit_diagnostics
changelog: `RequestHeaders.entries()` lists every request header as a `Map[String, String]` of lower-case names, a repeated field combined with `", "` (`Cookie` with `"; "`) and the route's consumed credential absent; experimental, behind `bynk.RequestHeaders.entries` in `[unstable] allow`
---

## ADR: request-header-enumeration
title: A handler enumerates the request's headers with `RequestHeaders.entries()`
summary: `entries() -> Map[String, String]`, lower-case names, repeats combined, the consumed credential hidden; experimental, as a built-in item

**Context.** `RequestHeaders` (ADR 0595) had `get`, `bearer` and `clientIp`, so a
handler could read a header it already knew the name of, but couldn't list them.
The pass-through HTTP track (`design/tracks/pass-through-http.md`, decision D12,
spine #3752) needs a list, so a proxy can forward a request's end-to-end headers.
It also answers #3752's Q6 for requests: does a multi-value form need to exist?
RFC 9110 §5.3 lets a recipient combine a field sent more than once into one
value joined with `", "`, and RFC 6265 joins cookie pairs with `"; "`. Either
join is lossless for every request header, so a request has no field that needs
a list per name.

There was also no way to mark a *method of a built-in type* experimental.
`[unstable] allow` names standard-library items by unit (`bynk.cloudflare.Kv.putTtl`),
and `RequestHeaders` belongs to no unit.

**Decision.** `RequestHeaders.entries() -> Map[String, String]` is every header the
view holds, keyed by lower-case name. A repeated field arrives once, joined as `get`
joins it. A test's `RequestHeaders.of` lines for `Cookie` are joined with `"; "`,
the way a browser sends one cookie-string, so the system-tier driver sends what a
real client would. The view is the same one `get` reads, so a credential the
route's actor consumed (ADR 0899: `Authorization` on a `Bearer` or `Oidc` route,
the signature header, a `SignedCookie` session cookie) is absent from `entries()`
too. On the response side, the track's `Forwarded` keeps the platform's own
headers, so a multi-value form for `Fetch` stays out of scope.

The method enters as experimental. `bynk-std` gains `BUILTIN_EXPERIMENTAL`, a list
of experimental items the language itself provides, each spelt under `bynk`
(`bynk.RequestHeaders.entries`). They count as first-party items (so an allow entry
naming one is valid) and as experimental items (so using one without the entry is
`bynk.stability.experimental`). The track's later built-ins (`PassThrough`,
`InboundRequest`, `Forwarded`) join the same list.

Rejected: `Map[String, List[String]]`, which matches `HttpResult.headers()` in tests
but makes every caller unwrap a one-item list for no request field that needs it;
`names()` plus `get`, which is two calls for one need; shipping the method stable,
against the track's rule that every new item enters experimental.

**Consequences.** A handler can list or forward a request's headers without knowing
their names, and a credential the seam consumed can't leak through the list. The
TypeScript shape of `RequestHeaders` gains `entries()`. Every emitted project that
mentions the type changes text, but behaviour is unchanged. A built-in item can now
ship experimental and be promoted later by removing it from `BUILTIN_EXPERIMENTAL`.
