---
level: patch
changelog: "`bynk test --mutate` also mutates `if` conditions, `match` arm guards and binary `+`/`-`"
---

## ADR: mutate-guards-and-arithmetic
title: `--mutate` guard, branch and arithmetic mutants
summary: Which sites slice 2 of `bynk test --mutate` mutates, and what "guard" means

**Context.** Slice 1 of #3069 mutated comparison operators and `&&`/`||`. The
proposal's slice 2 adds guard and branch mutants and `+`↔`-`.

**Decision.** Sites stay token-driven, like slice 1: an `if` keyword's condition
(up to the `{` or `=>` at bracket depth zero) is replaced by `true` and by
`false`, which covers both an `if` expression's branch and a `match` arm's `if`
guard; a `+` or `-` that follows an operand is swapped for the other. A mutant
the checker refuses (`+` on strings, a condition containing a `{`) is `invalid`
and unscored, as before. A `let … else` has no condition to drop (the `else`
runs when the pattern fails), so the proposal's "drop a `let … else` guard" is
read as the match-arm guard it desugars towards; the `let … else` pattern itself
is not mutated.

**Consequences.** More mutants per project, so `--only` and `--max-mutants`
matter more. No grammar, checker, emitter or runtime change.
