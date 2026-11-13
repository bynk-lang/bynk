---
level: minor
changelog: `bynk deploy` creates each `Vectorize` index with its declared dimensions and metric, reuses a matching one, fails (never recreates) on a mismatch, creates `@indexed` metadata indexes before the push, and reports orphaned indexes that `--prune` deletes behind a confirmation
probes: emit_abi_shapes, emit_diagnostics, ts_writes
---
