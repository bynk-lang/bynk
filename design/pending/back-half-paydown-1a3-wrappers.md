---
level: patch
probes: ts_writes
changelog: Wrapper and simple composite expressions (`Paren`, unary operators, `Ok`/`Err`/`Some`, list literals) now lower to typed TypeScript nodes; a negated negation keeps the `-(-x)` bracketing #3698 introduced, on both the node and the text path
---
