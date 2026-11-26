---
level: minor
probes: emit_diagnostics, diagnostic_coverage, explained_codes
changelog: `pattern as name` binds the whole value a pattern matched, so a pattern can check a value's shape and keep the value too
---

## ADR: pattern-as
title: `as` binds the whole value a pattern matched
summary: `pattern as name` is a suffix on any pattern, binds at the pattern's type, binds tighter than `|` and never changes what the pattern matches

**Context.** #3971. A pattern could match a value's shape but not keep the value: `Some(Order { status: Paid, .. } as paid)` was a parse error. The workarounds were to re-bind each field, or to bind the value and test its shape in an `if` guard, and a guarded arm never counts towards exhaustiveness, so the match then needed a `_` arm it had not needed before. Authors picked the guard and lost the check.

**Decision.** (A) `pattern as identifier`, a suffix on any pattern, binds the identifier at the static type of the position the pattern sits at: the sum for a variant pattern, the record for a record pattern, the element for a list element. Rejected: Rust's `name @ pattern`, which reads backwards beside Bynk's other `as` forms. `as` stays contextual, and only a pattern followed by `as` takes the new reading. (B) `as` binds tighter than `|`: `A | B as x` is `A | (B as x)`, which fails the or-pattern rule unless `A` binds `x` too, and `(A | B) as x` binds either alternative; the formatter keeps those parentheses. (C) Or-patterns bind the same names at the same types, as before. (D) Exhaustiveness and reachability are unchanged: the usefulness check reads `p as x` as `p`, and a duplicate arm is judged on the pattern inside. (E) It is allowed wherever a pattern is: a match arm, a payload, a record field, a list element, an or-alternative, `let … else`, an irrefutable lambda record pattern and after `is`. After `is`, where only the flat names of a position are bound, an `as` name is bound on the whole pattern or in place of a payload, field or element name, and a record pattern keeps its parentheses. `_ as x` is the warning `bynk.match.as_on_wildcard`, whose fix writes `x`.

**Consequences.** A new AST node, `Pattern::As`, carried through the resolver, the checker, the usefulness check, the emitter's `match` and `is` lowering and the formatter; the emitter lowers an arm with an `as` through its if-chain, never a `case` label. The tree-sitter grammar gains `as_pattern`. The editor shows the bound name's type on hover and colours it as a local, and the `as` keyword is highlighted as it is elsewhere; completion and signature help are unchanged. The refactoring that rewrites a guard-only arm as an `as` pattern is not part of this change.
