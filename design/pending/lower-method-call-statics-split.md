---
level: patch
changelog: Internal refactor of the emitter, with no change to emitted TypeScript. The intrinsic static calls of a method call (`HttpResult`, `List`/`Map`/`Set` constructors, the numeric parses, `Bytes`, `Decimal`, `Stream` and the rest) now lower in two functions instead of inline in `lower_method_call` (#3167).
---
