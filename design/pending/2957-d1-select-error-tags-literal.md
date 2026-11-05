---
level: patch
changelog: "A match on a typed D1 read's `Err(Sql(e))` / `Err(Decode(e))` now narrows `e`, instead of failing `tsc` with TS2345"
---
