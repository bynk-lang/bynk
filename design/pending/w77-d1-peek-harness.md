---
level: minor
probes: ts_writes, ts_any
changelog: A `system` or `integration` case can run `D1.peek` reads against the participant's in-memory test database, which refuses every write at run time (harness half)
---
