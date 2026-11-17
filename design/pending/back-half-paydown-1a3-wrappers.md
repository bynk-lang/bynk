---
level: patch
probes: ts_writes
changelog: Wrapper and simple composite expressions (`Paren`, unary operators, `Ok`/`Err`/`Some`, list literals) now lower to typed TypeScript nodes, and a double negation such as `- -x` now emits `-(-x)` instead of the invalid `--x`
---
