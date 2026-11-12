---
level: patch
probes: ts_writes
changelog: `Query.sum` takes a `Decimal` key and sums it exactly, as `List.sum` does, and `average` over a `Decimal` key says why it is refused and points at `sum` then `div`
---
