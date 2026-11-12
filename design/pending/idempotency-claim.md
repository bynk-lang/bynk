---
level: minor
changelog: `Idempotency.claim(key, expiresAfter)` is an atomic first-delivery-wins claim, and the test double honours `expiresAfter` on the harness clock
probes: emit_abi_shapes, ts_writes
breaking: The `Idempotency` test double now honours `expiresAfter` on the harness clock, so a case that `advance`s `Clock` past a `remember` window sees `dedup` return `None`, and a zero or negative `expiresAfter` lapses at once; before, the entry never lapsed.
---
