# 1084 — An API key seam refuses a malformed key before any lookup and every other bad key with one identical 401, and a store fault never admits

- **Status:** Accepted (v0.810.1)

**Context.** A seam that answers differently, or works differently, for an unknown key than for a wrong one tells an attacker which keys exist; one that admits on a store fault is a bypass; one that caches tells a revoked key it is still good (track §8, §13, Q4). The existing seams compare in constant time through `crypto.subtle.verify`, answer `Unauthorized` for every failure after their first check, and hide the credential they consume from the handler ([ADR 0899](0899-seam-consumed-credential-hidden-from-request-headers.md)).

**Decision A — the order of checks.** On a route an `ApiKey` actor guards, before the handler runs. The request is read and its parameters decoded first, in the order an ordinary route keeps (ADRs 1009 and 1023); only a `key: identity` rate limit splits verification ahead of the body, through ADR 1023 B's `verify` and `run` steps:

1. Read the key from the actor's `header`, or from `Authorization: Bearer` when there is none. Absent, or the header sent more than once: `401`.
2. Parse: length, prefix, the three parts, base62, checksum. Any failure is `401` **with no lookup**.
3. Compute `HMAC-SHA-256(pepper, secret)`, and under `previous` too while it is set.
4. Look up and compare in the agent: `<Actor>Keys(handle).verify(keyNo, digests)`. Then the expiry, judged in the agent against the `now` the seam passes. An expired record does the same comparison work and is refused before the agent writes anything, so no refusal re-hashes a record or records its use.
5. Mint the identity through the identity type's `of`; a violation is `401`.
6. Run the handler.

`Authorization: Bearer` is the default because it is what HTTP clients, proxies and log scrubbers already treat as a credential. Rejected: `x-api-key` as the default, which is not a standard and is not redacted by default in common logging.

**Decision B — one refusal, the same work.** Every refusal from step 2 onwards is the same `401`, with the same body and headers, whatever failed. An unknown key number is compared against a fixed dummy digest, as is a record whose `kid` matches neither pepper, so the work is the same as for a wrong secret; an unknown handle reaches an instance with no records and does the same work. The comparison is `bynk.crypto`'s constant-time equality, including the compare against the dummy. A malformed key is refused faster, which reveals only that the format is wrong, which is public. The `401` carries a `WWW-Authenticate` challenge: `Bearer realm="<context>"` for the default header, and `Bynk-ApiKey realm="<context>", header="<header>"` for a named one.

**Decision C — a store fault never admits and is never a `401`.** An unreachable agent faults the request (`500`), and `AgentBusy` answers `503` with `Retry-After: 1`, as on any HTTP route, so a client does not conclude its key is bad. In an actor sum, a store fault in an `ApiKey` arm faults the request; it never falls through to a later arm or to `Visitor`.

**Decision D — nothing is cached.** Every request asks the agent, so a revoked key is refused on the next request. The price is one agent round trip per authenticated request, and the reference states it.

**Decision E — the handler never sees the key.** The actor's header joins `consumed_credential_headers`, as ADR 0899 already anticipates for `ApiKey`, so it is removed from the handler's `RequestHeaders` view, and a typed `headers` field that reads it is `bynk.http.headers_field_consumed` ([ADR 0987](0987-refuse-consumed-header-fields.md)). In a sum, every member's header is hidden whichever arm resolves: a refused key may still be a live secret, and no handler needs it.

**Decision F — fabricated keys are bounded by a rate limit asked first, and its absence is warned.** A well-formed fabricated key is cheap to make and reaches the store, so the defence is `@rateLimit(Limiter, key: clientIp)`, asked before authentication ([ADR 1009](1009-rate-limit-annotation-client-ip.md) A). It must not be opt-in: an `ApiKey`-guarded route that applies no rate limit before authentication is the warning `bynk.actor.api_key_unlimited`, which says what an unlimited route costs. It is a warning, not an error, so a service rate-limited at the edge still compiles. `key: identity` ([ADR 1023](1023-rate-limit-identity-key.md)) does not help here, since a fabricated key never reaches that limiter. Rejected: an authenticated id (a MAC over the id), which would refuse fabricated ids without a lookup but tie every id to the pepper, so a rotation would end every key.

**Consequences.** The seam is fail-closed at the HTTP boundary, in generated code, and the handler sees only the sealed identity. Revocation is as fast as the next request, not faster: a request already past the seam completes. A never-used instance may start more slowly than a busy one, so timing can tell whether a handle has been used; handles are 128-bit random, so an attacker learns this only about a handle they already hold. An `ApiKey` actor on a WebSocket `on open` is `bynk.actor.scheme_not_admissible` until the upgrade has a liveness check, as for `sessions = true` ([ADR 1061](1061-websocket-cookie-oidc-open.md) A). Every refusal path is covered by a bypass-class test ([ADR 0087](0087-security-ci-posture.md)), and a harness test pins that the failure principals get byte-identical responses.
