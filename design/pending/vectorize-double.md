---
level: minor
changelog: In `bynk test` a `Vectorize` instance is an exact in-memory index built from its declaration (dimensions, metric, `@indexed`): it ranks by the platform's score semantics, keeps insert and upsert apart, partitions namespaces, applies all eight filter operators and the documented limits, and `stub` still replaces any operation; a write is visible at once, which production's asynchronous upserts are not
probes: emit_abi_shapes, emit_diagnostics, ts_writes
---
