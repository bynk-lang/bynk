---
level: minor
changelog: "Precise exhaustiveness and reachability: a Maranget usefulness engine replaces the conservative coverage rules, so a multi-field variant payload or a record is exhaustive when its arms cover every case between them (no `_` needed), every uncovered case is reported as a pattern you can paste as an arm (`Pair(false, false)`, `Order { status: Cancelled, .. }`), and redundancy spread across several earlier arms is now `bynk.types.unreachable_arm` (patterns track slice 3, [#2665](https://github.com/bynk-lang/bynk/issues/2665))"
---

## ADR: usefulness-based-exhaustiveness
title: Coverage is decided by one usefulness algorithm, superseding ADR 0169 D
summary: Exhaustiveness, reachability and their witnesses come from one Maranget usefulness engine over a constructor space (variants, records, `Bool`, unbounded literals) with a step budget and a sound fallback; supersedes ADR 0169 D's conservative multi-field rule and slice 1's record rule; realises patterns-track slice 3

**Context.** ADR 0169 D bounded coverage by hand: a single-field payload
recursed, but a multi-field payload with a refutable field counted as covered
only if one arm bound every field irrefutably, with the unwritable witness
`Pair(…)`. Slice 1 (#2641) extended that to records: a record scrutinee was
exhaustive only if some arm was all-irrefutable, so arms that covered every
`status` between them still needed `_`. Reachability (`pattern_covered`, #1656)
compared an arm to one earlier arm at a time, so redundancy spread across several
arms was never reported. Slices 4 and 5 (list and range patterns) cannot judge
their own coverage without a real algorithm.

**Decision.**

- **A. One algorithm.** `bynk-check/src/checker/usefulness.rs` implements
  Maranget's usefulness over a pattern matrix. Exhaustiveness asks which values
  no unguarded arm matches; reachability asks whether an arm is useful after the
  unguarded arms before it; witnesses come from the same recursion.
  `missing_patterns`, `pattern_covered` and the `MatchKind::Record` coverage case
  are removed. `check_match`, the `else match` coverage and the `let … else`
  checks all call it.
- **B. The constructor space.** A sum, `Result` or `Option` has its declared
  variants, with payload fields in declared order (named and positional bindings
  are the same columns). An extensible enum has one more constructor, the future
  variant, that only a wildcard or binding covers; its witness and
  `bynk.types.extensible_match_needs_wildcard` are unchanged. A record has one
  constructor whose columns are all its fields in declared order, and an unlisted
  field is a wildcard column, so patterns stay open (ADR 0669). `Bool` has `true`
  and `false`. `Int` and `String` are infinite: literals are constructors but no
  finite set of them is complete. A refined type uses its base; an opaque type has
  no constructors. `Constructor` is the extension point: list lengths (slice 4)
  and integer intervals (slice 5) add a constructor each.
- **C. Guards and refinements are as before.** A guarded arm is never a row, but
  its own reachability is still tested. A refined pattern as a prior contributes
  no row; as the arm under test it is tested as its inner pattern. Or-patterns,
  at any depth, expand into rows.
- **D. Witnesses are concrete.** A non-exhaustive `match` reports a pattern that
  can be written as an arm: `Pair(false, false)` for a multi-field payload,
  `Order { status: Cancelled, .. }` for a record (only the constrained fields,
  then `..`), `_` for an unbounded type. Witnesses the old engine already got
  right are textually unchanged. At most eight are reported for one `match`,
  followed by a note counting the rest; a capped match offers no add-missing-arms
  fix, since filling only some would leave it non-exhaustive.
- **E. Newly detected redundancy is `bynk.types.unreachable_arm`.** The rule that
  an arm MUST NOT be unreachable (#1656) is unchanged; the old engine merely
  missed some cases. The changelog states the tightening.
- **F. A step budget with a sound fallback.** Usefulness is exponential in the
  worst case, so each question counts its steps against a fixed budget, set well
  above anything in the corpus. When it runs out, exhaustiveness reports
  `bynk.types.non_exhaustive_match` with witness `_` and a note that the match is
  too complex to check precisely and wants a wildcard arm; reachability reports
  nothing.
- **G. The slice 2 fix is gated on irrefutability.** Precise coverage can prove a
  syntactically refutable record pattern covering (a field typed as a single-variant
  sum, say). `let … else` still reports `unreachable_arm` for such a pattern, but
  offers the machine-applicable "remove the `else`" fix only when
  `Pattern::is_irrefutable` holds, so the fixed code always re-parses.
- **H. `let` stays syntactic.** Whether a `let` may drop its `else` remains the
  parser's rule from slice 2 (ADR 0675). Precise coverage decides diagnostics,
  not what parses.

**Consequences.**

- Matches over multi-field payloads and records no longer need a `_` that hides
  later mistakes; adding a variant or a status breaks the `match` that should
  break.
- Some programs that compiled now report `bynk.types.unreachable_arm`, for an
  arm several earlier arms already cover together. Each is dead code; removing it
  does not change behaviour.
- Emission is untouched: an if-chain already ends in a `throw`, so a match the
  checker newly accepts without `_` lowers as before.
- Slices 4 and 5 add a `Constructor` and teach the space to split it; there is
  no second algorithm.
