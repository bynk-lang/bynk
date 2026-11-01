# 0669 — Record patterns are always open, and end with a required `..`

- **Status:** Accepted (v0.455)

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
