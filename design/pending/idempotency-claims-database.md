---
level: minor
changelog: A Worker that calls `Idempotency.claim` declares an implicit D1 database for its claims, which `bynk deploy` and `bynk dev` provision with no author action
probes: emit_abi_shapes, ts_writes
---
