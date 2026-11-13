# 0836 — Vectorize is an instance kind with a fixed index shape and a typed filter

- **Status:** Accepted (v0.592)

**Context.** An index's dimensions and metric cannot change after creation, the index is project-owned state, and writes are asynchronous.

**Decision.** Records decisions A–K of #3479. A: `Vectorize` is a named instance with
required `@dimensions` and `@metric` (a closed set), because an index's
configuration cannot change after creation. B: the deploy creates the index
and never recreates it, and a mismatch fails the deploy (slice V2). C:
metadata indexes come from `@indexed` (slice V2 creates them). D: metadata is
JSON text on reads and typed on writes (`upsertValue`, slice V4). E: typed
reads are not used, since one stale vector would fault every query. F: the
filter is a closed typed list of conditions. G: errors are values, with the
`Vector` prefix on `VectorInvalidInput`, `VectorRateLimited` and
`VectorFailed` because `InvalidInput`, `RateLimited` and `Failed` belong to
other error types. H: writes return a `Mutation`. I: `bynk dev` binds only a
named index (V2). J: the in-memory exact test double (V3); until then every
operation faults until stubbed. K: pairing with `Ai.embed` is documentation.

**Consequences.** Experimental, so no 1.0 promise. A program that does not use `Vectorize` has unchanged handler and Worker output; only the emitted `bynk.cloudflare` module grows. The deploy, `bynk dev` and the in-memory double follow in later slices.
