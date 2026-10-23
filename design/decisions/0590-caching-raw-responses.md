# 0590 — Caching raw responses

- **Status:** Accepted (v0.393)

**Context.** ADR 0163 gave a `GET` returning JSON `Ok` an automatic weak `ETag`
(FNV-1a over the serialised body), a router-synthesised `304` on a matching
`If-None-Match`, and the opt-in `@cache(maxAge:, scope:)` freshness window. Its
D2 deferred `Raw` (ADR 0143): "a `Raw` body could be hashed but is deferred". So a
`Raw` response carried no validator and was never answered `304`, and
`Cache-Control` had no `immutable`. The first-party registry
(`design/tracks/first-party-registry.md`, slice L3) serves only `Raw` —
`config.json`, index files, and immutable archives of up to 64 MiB — and §3.2
expects its read path to be cached at the edge: archives cached for good, index
files short-lived and revalidatable. This resolves ADR 0163 D2's deferral and adds
a variant to ADR 0126's `HttpResult` vocabulary.

**Decision.**

- **A. A `Raw` `GET` body of at most 1 MiB gets the automatic weak `ETag`.** The
  same FNV-1a, over the body's octets, in the same `W/"<base36>"` form, attached
  where the `GET` dispatch already passes `weakEtag`; the existing
  `notModifiedIfMatch` answers `304`. Above 1 MiB (1,048,576 bytes) there is no
  automatic `ETag`: the hash is synchronous and runs on every request, `304`s
  included, in proportion to the body, so a large body gets a validator only from
  the handler (B). Rejected: hashing every `Raw` body (unbounded CPU per request);
  SHA-256 through Web Crypto (asynchronous, which would make the response path
  asynchronous for a weak validator's job — ADR 0163 D3's reasoning).
- **B. `RawTagged(body: Bytes, contentType: String, etag: String)`, a new 200
  variant.** A new `HTTP_VARIANTS` row and `HttpVariantPayload::RawTagged` shape,
  with the exhaustive arms (construction check, the `variants_of` binding with field
  names `body`/`contentType`/`etag`, the emitter's payload-field names). The `etag`
  is sent as the `ETag` exactly as given, with no hashing whatever the body's size,
  on any method; only a `GET` goes through the `304` step. It is checked when the
  response is built against RFC 9110 §8.8.3's `entity-tag` (`[W/]"…"`, `etagc` =
  `%x21 / %x23-7E`; `obs-text` is left out so every accepted tag is a valid header
  value). A malformed tag throws inside the dispatch's `try`, so the route answers
  the entry's fault `500` and the Worker logs `<context> <route> faulted` with the
  reason: the handler is named by its route, and a broken header is never sent. The
  check is at run time because a tag is usually built from a value (a content hash,
  an object store's `etag`). Rejected: a field on `Raw` (breaks every `Raw(…)`
  call, and `Raw` stays the zero-ceremony form); an options record on `Raw` (the
  same break, for one optional field).
- **C. `@cache` gains `immutable`.** `@cache(maxAge:, scope:, immutable: true)`
  emits `Cache-Control: <scope>, max-age=<n>, immutable` (RFC 8246); it defaults to
  `false`, and a non-`Bool` literal is `bynk.http.cache_bad_immutable`.
  `CacheIr` carries `immutable`, and the dispatch passes `applyCache` a fourth
  argument only when it is `true`, so every existing `@cache` route emits what it
  did. `applyCache` already stamped any response, so `@cache` on a `Raw` `GET`
  already emitted `Cache-Control`; it is now documented and tested: `@cache`
  applies to `Ok`, `Raw` and `RawTagged`, and is ignored on `Streaming` and errors.
  The static check for `@cache` on a streaming handler stays ADR 0163 D2's
  follow-on.
- **D. `304` stays a router response, and nothing changes for `Ok`.**
  `notModifiedIfMatch` handles the new validators unchanged (weak comparison,
  `ETag` and `Cache-Control` copied onto the `304`), and a `HEAD` carries its
  `GET`'s `ETag` as it carries every `GET` header.
- **E. Tests.** Runtime unit tests pin the 1 MiB boundary (exactly 1,048,576 bytes
  is tagged, one more is not), the tag grammar, and `immutable`. A system-tier
  behaviour test (`http_raw_caching_behaviour`, over the `2335_http_raw_caching`
  fixture) drives the Worker: a small `Raw` `GET` and its `304`, a 2 MiB body with
  no `ETag`, a `RawTagged` tag and its `304`, `immutable`, `HEAD`, and the `500` for
  a malformed tag. A unit-tier suite matches `RawTagged(body, ct, etag)`. The
  issue expected existing `Raw`-returning `GET` goldens to regenerate with
  header-only deltas; there are none, because every `GET` dispatch already passed
  `weakEtag` and the `Raw` decision lives in the runtime, which no fixture's
  `expected/` holds.

**Consequences.** The registry's archives can be served with a content-hash `ETag`
and `public, max-age=31536000, immutable`, and its index files with a short
`max-age` and a revalidatable weak `ETag`; any service that serves files gets the
same. A `304` still runs the handler (the body must exist to hash, or the handler
returns the tag after reading it): a validator answered before the handler reads
the body needs request-header access in the handler, a separate surface.
`Last-Modified`/`If-Modified-Since`, byte ranges (the one place a strong validator
is required), and caching `Streaming` responses are not part of this. A wrong
`immutable` is the author's broken promise: caches serve the stale response until
`max-age` expires. Completion offers `immutable` inside `@cache(…)`, `RawTagged`
comes from the variant registry, and hover on `@cache` shows the emitted
`Cache-Control`.
