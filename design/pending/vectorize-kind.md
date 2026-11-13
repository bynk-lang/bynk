---
level: minor
changelog: `bynk.cloudflare` gains `Vectorize`, an instance kind for a Vectorize index with required `@dimensions` and `@metric`, optional `@indexed`, a typed filter and typed errors, emitted as `[[vectorize]]`; every operation faults until stubbed in a test
probes: emit_abi_shapes, emit_diagnostics, ts_writes
---

## ADR: Vectorize as an instance kind

Records decisions A–K of #3479. A: `Vectorize` is a named instance with
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
