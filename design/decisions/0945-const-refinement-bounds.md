# 0945 — A constant stands as a refinement predicate's argument under ADR 0826's rule

- **Status:** Accepted (v0.683)

**Context.** A refinement bound is the commonest place a limit is written, and the one place ADR 0826's constant substitution did not reach: `InRange(1, 100)` had to repeat the literal wherever the limit was also needed (an error message, a test, a `@limit`).

**Decision.** The parser keeps a constant it meets in a predicate argument as a name (`PendingPred`); it has no constant table. The resolution pass that already rewrites annotation arguments substitutes the literal a constant is bound to and builds the predicate's real kind, ahead of every reader, so literal admission, boundary subsumption, OpenAPI and the property-test generator see a literal. The rule is ADR 0826's, unchanged: the constant is declared exactly the argument's type (`Int` for a length, the base type for a range or one-sided bound, `String` for `Matches`) and bound to a literal or to another such constant, possibly from a `uses`d commons. Anything else is `bynk.refine.bound_not_literal`. A constant declared the refined type whose bound it is is `bynk.resolve.const_cycle`. Arbitrary static expressions (`MaxQty + 1`) are rejected: they would need constant folding the language does not have. Emitter, runtime and wire are untouched.

**Consequences.** One new diagnostic, explained and fixtured. The formatter keeps the constant's name. Hover on a bound already reaches the constant (the reference is recorded as a use), and a refined type's hover shows the predicate with the values substituted. Completion inside a predicate argument offers the in-scope constants of the type that argument takes.
