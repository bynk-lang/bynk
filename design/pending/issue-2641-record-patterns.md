---
level: minor
probes: emit_diagnostics, ts_writes, diagnostic_coverage
changelog: "Record patterns: `TypeName { f1, f2: pattern, .. }` matches a record by its fields in a `match` arm, nested in a variant payload, or after `is` (parenthesised, `x is (T { .. })`). They are always open, so the trailing `..` is required and adding a field to a record never breaks a pattern. A record pattern covers its type only when every field is matched irrefutably (patterns track slice 1, [#2641](https://github.com/bynk-lang/bynk/issues/2641)). New diagnostics: `bynk.parse.record_pattern_without_rest`, `bynk.parse.record_pattern_in_is_unparenthesised`, `bynk.types.record_pattern_on_non_record` and `bynk.types.duplicate_pattern_field`, the two parse errors with one-click fixes"
---

## ADR: open-record-patterns-with-required-rest
title: Record patterns are always open, and end with a required `..`
summary: `TypeName { f1, f2: pattern, .. }` matches a record by its fields in `match` and `is`; the type name and the trailing `..` are required, so adding a field never breaks a pattern; coverage is conservative, visibility is a field read's, and lowering is the if-chain; realises patterns-track slice 1

**Context.** A record can only be read through `.field`. A variant payload
pattern stops at the first record it meets (`Some(order)` binds the whole
record), so matching a record's fields takes nested `if`s or a `match` on one
field at a time. ADR 0169 F named record patterns "the natural next slice" and
reused the recursive `Pattern` node. The patterns track (`design/tracks/patterns.md`,
spine #2616) settled the open questions for this slice: Q1 (open or closed
patterns), Q2 (syntax), Q3 (visibility) and Q4 (exhaustiveness).

The load-bearing constraint is that **adding a field to a record must not break
a pattern**. The open proposal for record field defaults (#2607) is built on
it, and so is the Foundations-layer stability promise.

**Decision.**

- **A. The surface is `TypeName { f1, f2: pattern, .. }`.** The type name is
  required and carries no type arguments (a generic record's come from the
  scrutinee, as in construction). A bare `f` binds the field to a local `f`;
  `f: p` matches the field against any sub-pattern, exactly as a variant
  payload's `named_binding` does. Record patterns nest anywhere a pattern can,
  and inside or-patterns.
- **B. Record patterns are always open, and `..` is required** (track Q1 (c)).
  Unlisted fields are ignored whether or not the pattern lists them all, so no
  record pattern can be closed and no added field breaks one. The rule is the
  one the events track's payload pattern already applies. Writing the pattern
  without the `..` is `bynk.parse.record_pattern_without_rest`, a parse error
  carrying a machine-applicable fix that inserts `, ..`. The cost is two
  characters on a pattern that happens to list every field; the alternative,
  an optional `..` (Q1 (a)), would give the same pattern two spellings and
  disagree with events.
- **C. Type checks.** The named type MUST be a record type
  (`bynk.types.record_pattern_on_non_record`: a sum, enum, refined or opaque
  type, or a built-in) and the scrutinee MUST be that record
  (`bynk.types.pattern_type_mismatch`). Each field MUST exist
  (`bynk.types.unknown_field`) and appear at most once
  (`bynk.types.duplicate_pattern_field`). Or-pattern alternatives bind the same
  names at the same types, under the existing rule. A `match` on a record is
  accepted when an arm has a record pattern; a `match` on a record with none
  keeps `bynk.types.match_non_sum_discriminant`.
- **D. Visibility is a field read's** (track Q3). A record pattern on a type the
  unit may not inspect is `bynk.context.opaque_inspection`, reported at the
  pattern, so a pattern can never read a field `.field` could not. An opaque
  *type* is not a record and so has no record pattern at all; `.raw` stays the
  only way in. A type the consumer can read but cannot name has no pattern
  either, since the type name is required; such values are still read with
  `.field`.
- **E. Coverage is conservative** (track Q4 (a), ADR 0169 D extended to
  records). A record pattern covers its type only when every field sub-pattern
  is irrefutable; otherwise it is partial, and a later irrefutable arm is
  needed for the `match` to be exhaustive. `bynk.types.unreachable_arm` uses
  the same rule, and additionally reports an arm that one earlier record
  pattern covers field by field. Precision across several arms is a later
  slice (track Q4 (b)).
- **F. Lowering is the existing if-chain.** A record pattern has no tag, so any
  `match` with one lowers as a sequence of `if` blocks: each arm tests its
  refutable field sub-patterns against `scrutinee.field`, binds the bound
  fields as `const`, then runs the guard and body. Flat `switch` matches are
  unchanged, and the corpus emits byte-identically. An arm that tests nothing
  declares its bindings in its own block when an earlier such arm already
  declared the name, so two such arms (`Order { total, .. } if … =>` then
  `Order { total, .. } =>`) no longer redeclare it. `is` lowers through the
  same tests, and binds the pattern's flat names.

**Consequences.**

- Matching a record's fields is one pattern, and adding a field to a record
  breaks no existing pattern: the fixture `2641_record_patterns_added_field`
  shows it.
- Every record pattern pays the two characters of `..`; the parser says so, with
  a fix, rather than guessing.
- Exhaustiveness is weaker than a person would write until the precise
  algorithm lands: `Order { status: Paid, .. }` and `Order { status: Open, .. }`
  together are not seen to cover `Order` where `Status` has two variants, and
  the diagnostic suggests a `_` arm.
- Record patterns are not accepted in `let` yet (destructuring is slice 2), in
  lambda parameters or in function parameters.

## ADR: record-pattern-parenthesised-after-is
title: A record pattern after `is` is parenthesised
summary: `x is (T { f, .. })` needs parentheses, because the `{` would also open the `if`'s block; the parser tells the two apart by scanning for a closing `..`, and offers a fix; both parsers reject the unparenthesised form

**Context.** After `is`, a pattern can be followed by a block: `if o is Item
{ … }` already means a unit variant `Item` and a then-block. A record pattern
`Item { sku, .. }` begins the same way, so at the first `{` neither parser can
tell whether it opens the pattern or the block. Rust has the same problem with
struct literals in an `if` condition and bans them there. The track (Q2)
considered three answers: parenthesise after `is`, a lookahead rule, or no record
patterns in `is`.

**Decision.**

- **A. A record pattern after `is` MUST be parenthesised:**
  `if o is (Order { status: Cancelled, .. }) { … }`. It uses the existing
  parenthesised pattern (ADR 0252), so no new production is needed. The
  restriction covers a record pattern at the tail of an unparenthesised `is`
  pattern, at the top level or as an or-alternative. Inside any delimiter (a
  variant payload, parentheses, another record pattern) a record pattern needs
  nothing, and in `match` arms it is never parenthesised.
- **B. The tree-sitter grammar excludes the bare form structurally:** `is_expr`
  takes an `_is_pattern`, which is `_pattern` without a record pattern at its
  tail. The hand-written parser carries a flag for the same position, and, to
  say something useful about the unparenthesised form instead of a bare "expected
  `{`", scans to the matching `}` and reports
  `bynk.parse.record_pattern_in_is_unparenthesised` when the brace group ends in
  `..`. A block never ends in `..` (it is not an expression), so
  `if x is Foo { y }` is still a variant and a block. The error carries a
  machine-applicable fix that adds the parentheses. Both parsers reject the
  form, which the cross-parser conformance test pins.
- **C. `let … else` needs no such rule,** because there the pattern ends at `=`,
  not at a block. This ADR does not add record patterns to `let`; a
  parenthesised one parses there only as a side effect of the parenthesised
  pattern, and destructuring in `let` is a later slice.

**Consequences.**

- One simple, local rule with no unbounded parsing, and `match` arms and nested
  positions stay unparenthesised.
- The formatter re-adds the parentheses, since the parser treats them as
  transparent grouping.
- The cost is a pair of parentheses after `is`, and a one-click fix when they
  are forgotten.
