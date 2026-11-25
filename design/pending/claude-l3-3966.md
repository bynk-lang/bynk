---
level: minor
changelog: `?` on an `Option` returns `None` early in a function that returns `Option` (or `Effect[Option]`), and in a lambda that returns one
---

## ADR: question-on-option-in-option-fn
title: ? on an Option returns None early where the body returns an Option
summary: Same-type propagation only: Option to Option, never Option to Result, with the HttpResult lifting unchanged

**Context.** `?` on an `Option` was accepted only in a handler returning `HttpResult`, where `None` becomes `NotFound` (ADR 0177). Everywhere else it was `bynk.types.question_option_outside_http`, including a function that itself returns `Option`, so an `Option` chain needed a `match`, a `let … else` or nested `andThen` calls where the `Result` equivalent is a line per step.

**Decision.** In a body returning `Option[_]` or `Effect[Option[_]]`, `o?` has the payload type and returns the `None` itself on absence. Propagation is same-type only: an `Option` never converts to a `Result`, or the reverse, and a `Result` operand in an `Option` body stays `bynk.types.question_outside_result`. The `HttpResult` lifting is unchanged and is tried first. A lambda follows its own return type and the early return leaves the lambda, as for `Result`; `bynk.par.question_in_join` already covers an `Option` operand, since it is syntactic. In a `Result`-returning body the refusal stays, and gains a fix, `.okOr(<error>)`, with the error left as a placeholder (so it is offered, never applied by a fix-all). The emitter returns the operand when its tag is `None`, rather than `HttpResult.NotFound`, when the enclosing return type is an `Option` or an `Effect` of one.

**Consequences.** No grammar change and no runtime change. Hover on the `?` of an `Option` now names both propagations (the value or an early `None`, and `404 NotFound` in an `HttpResult` handler) instead of only the `404`: naming the one that applies would need the enclosing return type, which the editor's tables do not carry. A lambda's return type is known only where its expected function type is constrained, so a lambda whose return type is still being inferred keeps the refusal, exactly as `?` on a `Result` does today.
