---
level: patch
changelog: "`?` on a `Result` inside a test `case` fails the case, naming the `Err` and the `?`'s location, instead of emitting a `return` that failed `tsc` (#2857)"
probes: ts_writes
---

## ADR: question-fails-the-case
title: `?` in a test case fails the case
summary: An Err reached by `?` in a case body fails the case rather than propagating

**Context.** A `case` body is checked as returning `Effect[Result[(), ValidationError]]`, so `?` on a `Result` type-checked. The emitter lowered it as an early `return` of the `Err`, but the emitted case function returns a `CaseResult` (`{ pass, error? }`), so the output failed `tsc` (TS2345). The alternative to supporting `?` was a diagnostic refusing it.

**Decision.** In a unit or integration case body, an `Err` reached by `?` fails the case: it lowers to a `__bynkExpectFailure` throw carrying the `?` expression, the error value (`__bynkShow`) and the `?`'s `file:line:col`, the same report a failed `expect` gives. This applies at any nesting inside the case (`if`, `match`, `let … <-` sequences). A `?` inside a lambda defined in the case keeps its ordinary meaning. The error type remains `ValidationError`, as the checker already requires; `Option` operands stay refused outside `HttpResult` handlers.

**Consequences.** `Day.of("…")?` is a natural way to build fixtures in a test. No checker or grammar change. Widening the permitted error type beyond `ValidationError` is a separate decision.
