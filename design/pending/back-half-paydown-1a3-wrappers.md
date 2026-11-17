---
level: patch
probes: ts_writes
changelog: Wrapper and simple composite expressions (`Paren`, unary operators, `Ok`/`Err`/`Some`, list literals) lower to typed TypeScript nodes when their operands do, with no change to emitted output
---
