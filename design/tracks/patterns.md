# Destructuring and richer patterns — record, list and range patterns

- **Status:** Draft (settling). Direction not yet merged; no slice authorised.
  Spine [#2616](https://github.com/bynk-lang/bynk/issues/2616). The questions in
  §5 carry *draft* recommendations for the settling review; none is settled
  until this PR is marked ready and merged.
- **Realises:** the pattern grammar of `design/bynk-type-system.md` §2.3.4, and
  the next slice [ADR 0169](../decisions/0169-nested-payload-patterns-and-match-arm-guards.md)
  F names: "Field patterns on a record type (`Status { name, ok }`) are a
  distinct destructuring surface … They are the natural next slice and reuse
  exactly the recursive `Pattern` node this increment builds."
- **Posture:** Feature track per [ADR 0076](../decisions/0076-feature-track-posture.md).
  It qualifies on two axes (§2): it spans several increments, and its surface is
  not settled. One of its calls (Q1) decides whether adding a record field can
  ever break source code, which is a Foundations-stability property.
- **Governing records:** ADR 0158 (literal patterns), 0169 (nested payload
  patterns, guards, and the conservative exhaustiveness rule for multi-field
  payloads), 0252 (or-patterns), 0253 (refined patterns), 0484 (`let … else` is a
  block-scoped `match`); the events track's payload pattern
  (`{ field: v, .. }`, `design/tracks/events.md`); the open proposal #2607
  (record field defaults); events slice 4b #990 (range dispatch).

## 1. The theme

Bynk's patterns (§4.7) are a wildcard, a literal, a binding, a variant with
nested payload patterns (ADR 0169), an or-pattern (ADR 0252), and the
match-arm-only refined pattern `_ where …` (ADR 0253). `let … else` (ADR 0484)
brought refutable patterns to `let`. What is missing:

- **Record patterns.** A record is read only through `.field`. There is no
  `match o { Order { status: Paid, total, .. } => … }` and no
  `let Item { sku, qty, .. } = item`.
- **List patterns.** A list is inspected through `first()`, `get(i)` and
  `length()`. There is no `[]`, `[x]` or `[head, ..rest]`.
- **Range patterns.** `0..9 =>` does not exist. The nearest form is
  `_ where InRange(0, 9)`, which binds nothing. Range syntax is planned only for
  event schema dispatch (#990).

### 1.1 What this track settles

1. **Whether record patterns are open or closed** (§5 Q1), which decides whether
   adding a field to a record can break a pattern. #2607 currently relies on
   there being no record patterns at all: "No record patterns … So no exhaustive
   record pattern can break when a field is added."
2. **The syntax**, and how it avoids the `{` that already opens blocks, record
   literals and event patterns (Q2).
3. **Visibility**: a record pattern is field inspection and must obey the same
   rules as `.field` (Q3).
4. **Exhaustiveness** for multi-field patterns, where today's checker is
   deliberately conservative (Q4).
5. **Destructuring outside `match`**: plain `let`, and whether lambda or function
   parameters destructure (Q5).
6. **List patterns and range patterns** (Q6, Q7).

**End state when this track retires:**

- records can be matched and destructured by field, in `match`, `is`, and `let`;
- adding a field to a record, defaulted or not, never breaks a pattern;
- lists can be matched by shape (`[]`, `[x, ..rest]`) with exhaustiveness
  checking;
- integer range patterns work in `match`, with the same syntax as event schema
  dispatch;
- exhaustiveness checking for multi-field patterns is precise, not conservative.

## 2. Why a track (the ADR 0076 trigger)

- **Multi-increment.** Record patterns, `let` destructuring, precise
  exhaustiveness, list patterns and range patterns are each a grammar, checker,
  exhaustiveness and emission slice.
- **Surface not settled.** Open versus closed record patterns, the rest marker,
  list exhaustiveness and range syntax are all open (§5), and the range syntax
  must be agreed with events slice 4b (#990).
- **Not a security boundary**, though Q3 must not open a hole in the
  opaque-inspection rule.

## 3. What's already true

- **The pattern AST is recursive.** `enum Pattern` (`bynk-syntax/src/ast.rs`)
  has `Wildcard`, `Binding`, `Literal`, `Variant` (with nested payload
  patterns), `Or` and the refined form. A variant payload can already bind a
  field by name, `field: pattern` (`named_binding`, §4.7.6), which a record
  pattern's field syntax can reuse.
- **Lowering.** Flat matches emit a `switch`; nested or guarded ones emit an
  if/else chain (`match_needs_if_chain`, `bynk-emit/src/emitter/lower.rs`). A
  record pattern is a set of field tests, so it slots into the if-chain.
- **Exhaustiveness is conservative for multi-field payloads.**
  `missing_patterns` (`bynk-check/src/checker/expressions.rs`) recurses into a
  single-field payload, but "a multi-field refutable payload is conservatively
  reported uncovered unless a full (all-irrefutable) arm exists" (ADR 0169
  DECISION D). A record pattern is multi-field by nature.
- **There is already a partial record pattern.** An event subscription's payload
  pattern is `{ field: value, …, .. }` (`event_pattern`,
  `tree-sitter-bynk/grammar.js`), and the trailing `..` is **mandatory**. The
  events track notes that `..` "exists only as a record-pattern rest marker".
- **`let` patterns must open as patterns.** A pattern `let` must start with a
  literal, `(`, `Ok`/`Err`/`Some`/`None`, or a variant followed by `(` or `.`
  (`_let_pattern`, §4.8.4a), because a bare name after `let` is always a
  binding.
- **Field visibility.** Reading a field of an `exports opaque` type from another
  context is `bynk.context.opaque_inspection` (§6.5). An opaque *type* has no
  record syntax at all; its representation is reachable only through `.raw`
  inside its defining `commons`.
- **Record construction** always names the type: `Item { sku: "a", qty: 1 }`
  (§4.6.11). Spread may omit it.

## 4. Constraints on any answer

- **Adding a field must not break a pattern.** This is the property #2607 is
  built on, and one the Foundations-layer stability promise benefits from.
- **Patterns obey visibility.** Anything `.field` may not read, a pattern may not
  read either.
- **No new ambiguity with `{`.** Blocks, record literals, spreads, event
  patterns and `match` bodies all use braces already.
- **Existing matches emit unchanged.** Flat `switch` lowering and every current
  golden stay byte-identical.
- **Cost visible.** A pattern that copies data (a list rest) should say so in the
  spec.

## 5. Open design questions

Each question has options, prior art, and a **draft recommendation** for the
settling review to test.

### Q1 — Open or closed record patterns, and the `..` marker

- **(a) Always open, `..` optional.** Unlisted fields are ignored whether or not
  `..` is written. Prior art: Elm, Gleam, JavaScript destructuring.
- **(b) Closed unless `..` is written** (Rust). A pattern that lists every field
  without `..` breaks when a field is added.
- **(c) Always open, `..` required.** Every record pattern ends with `..`, as an
  event payload pattern already must.

**Draft recommendation: (c).** It keeps the #2607 property by construction (no
record pattern can ever be closed), it makes the "and the rest" visible at every
site, and it is the rule Bynk already applies to the only record-shaped pattern
it has. The cost is two characters on a pattern that happens to list every
field. (a) is the main alternative: equally safe, less noisy, but a second
spelling of the same pattern, and inconsistent with events.

### Q2 — Syntax

- **Type name required:** `Order { status: Paid, total, .. }`. It reads like
  construction, is the form ADR 0169 F uses, and the leading uppercase name
  followed by `{` cannot be confused with a block or with an event pattern (which
  sits inside `from Events(…)` and has no type name).
- **Fields:** `name` binds the field to a local of the same name (shorthand);
  `name: pattern` matches the field against a sub-pattern, exactly as
  `named_binding` does in variant payloads; `name: _` is allowed but redundant
  given `..`.
- **Generic records:** type arguments are not written in patterns
  (`Page { items, .. }`), as they are not written in construction.
- **Draft:** as above. The settling review should confirm there is no parse
  conflict in a `match` arm position, where `Order {` currently cannot start an
  arm at all.

### Q3 — Visibility

- A record pattern on a type the current unit may not inspect is rejected with
  the existing code, `bynk.context.opaque_inspection`, at the pattern.
- Opaque types are not records, so they have no record pattern; `.raw` remains
  the only way in, inside the defining `commons`.
- **The unnamed case.** A type the consumer cannot *name* but may read
  ("transparent but unnamed", §6.5) cannot appear in a pattern under Q2's
  type-name-required rule. **Draft:** accept that limitation. Such values are
  still read with `.field`, and allowing an unnamed form would reopen Q2's
  ambiguity question.

### Q4 — Exhaustiveness

- **(a) Keep the conservative rule.** A record pattern counts as covering only
  when every field sub-pattern is irrefutable; otherwise a wildcard arm is
  required. This is ADR 0169 D extended to records.
- **(b) A precise usefulness algorithm** (the pattern-matrix approach of
  Maranget's "Warnings for pattern matching", used by Rust, OCaml and others) for
  record, variant-payload, list and range patterns together.

**Draft recommendation: (a) in slice 1, then (b) as its own slice.** (a) is
sound and lets record patterns ship without touching the existing checker; (b)
then improves *every* multi-field case, including today's variant payloads, and
makes list and range exhaustiveness possible. (b) also feeds the existing
`unreachable_arm` check, which today can miss redundancy across multi-field
arms.

### Q5 — Destructuring in `let`, lambda and function parameters

- **`let`.** An irrefutable record pattern (every field sub-pattern irrefutable)
  may appear in a plain `let` with no `else`: `let Item { sku, qty, .. } = item`.
  A refutable one needs `else` as today (ADR 0484). `_let_pattern` gains
  "a type name followed by `{`" as a pattern opener.
- **Lambda parameters.** `(Item { sku, .. }) => sku` is convenient in
  `map`/`filter`. It needs an irrefutable pattern and a known parameter type.
- **Function parameters.** Destructuring in `fn` signatures would put patterns
  into signatures, hover, signature help and the cross-context contract.
- **Draft:** `let` in slice 2; lambda parameters as an optional later slice if
  usage asks for it; function parameters a non-goal, so a signature's parameters
  stay plain names.

### Q6 — List patterns

- **Forms:** `[]`, `[p₁, …, pₙ]` (exact length), and `[p₁, …, pₙ, ..rest]`
  (at least *n*, the rest bound to a `List`). A suffix form (`[.., last]`) is
  left out of the first slice.
- **Exhaustiveness:** by length. `[]` plus `[x, ..rest]` is exhaustive. This
  needs Q4 (b).
- **Cost:** binding `..rest` copies the remaining elements (`slice`), which is
  O(n) per match. The spec states it, and `..` without a name (`[x, ..]`) binds
  nothing and copies nothing.
- **Recursion:** list patterns invite recursive processing (`[x, ..rest] =>
  f(rest)`), which the spec does not yet specify (#2612). The kernel folds remain
  the recommended iteration; the reference should say so.
- **Draft:** as above, in a slice after Q4 (b).

### Q7 — Range patterns

- **Syntax shared with #990.** Inclusive `lo..hi`, and open-ended `lo..` and
  `..hi`. Inclusive matches `InRange(lo, hi)`, which is inclusive at both ends,
  so the two forms agree. The events track's `via schema(v1..v2)` should use the
  same production.
- **Types:** `Int` (including refined `Int`) only in the first slice. `Duration`
  and `Instant` have no literal form; `Float` ranges invite `NaN` and
  representation questions; `String` ranges are rarely meant.
- **`..` is reused.** In a record or list pattern `..` is a rest marker; with a
  literal on either side it is a range. The two never occur in the same
  position.
- **Relation to refined patterns.** `_ where InRange(0, 9)` matches the same
  values without binding. Ranges compose with or-patterns (`0..9 | 90..99`) and
  nest inside variant and record patterns (`Some(0..9)`). Binding the matched
  value as well would need an as-pattern (`n @ 0..9`), which is out of scope
  (§10); a plain binding with a guard covers that case. The refined form stays
  for predicates that are not ranges.
- **Exhaustiveness:** interval coverage over `Int`, once Q4 (b) exists.
- **Draft:** as above, cut jointly with #990 so both use one production.

### Q8 — Tuple patterns

The concurrent-effects track (#2615) recommends a binding block rather than
tuples for joining effects. If that holds, nothing in the language needs tuples
and this slice is dropped. **Draft:** keep it listed as conditional and drop it
when #2615 settles Q1.

## 6. Candidate slice decomposition

Provisional until this doc settles. Each becomes an increment proposal opened as
a sub-issue of #2616.

- **Slice 1 — Record patterns in `match` and `is`.** Q1–Q3; conservative
  exhaustiveness (Q4 (a)); if-chain lowering.
- **Slice 2 — Record destructuring in `let`.** Q5; the `_let_pattern` opener.
- **Slice 3 — Precise exhaustiveness.** Q4 (b) for every multi-field pattern,
  replacing ADR 0169 D's conservative rule and strengthening `unreachable_arm`.
- **Slice 4 — List patterns.** Q6, on top of slice 3.
- **Slice 5 — Range patterns.** Q7, jointly with #990.
- **Slice 6 — Lambda-parameter destructuring**, only if usage asks for it.
- **Slice 7 — Tuple patterns**, only if #2615 adopts tuples.

## 7. Front-loaded ADR candidates

To land with, or just before, slice 1:

- **Open record patterns with a required `..`** (Q1). It fixes whether adding a
  field can ever be source-breaking, and binds #2607's reasoning.
- **Range syntax shared with events** (Q7). Decided before either slice 5 or
  #990 lands, so the language gets one range production.

## 8. Threat model

None, because patterns are pure and static. The one safety-relevant rule is Q3:
a pattern must never read a field that `.field` could not. It reuses
`bynk.context.opaque_inspection` rather than adding a parallel check, and slice 1
needs a negative fixture for a pattern on an `exports opaque` type from another
context.

## 9. Slice status

- [ ] Slice 1 — record patterns in `match` and `is`
- [ ] Slice 2 — `let` destructuring
- [ ] Slice 3 — precise exhaustiveness
- [ ] Slice 4 — list patterns
- [ ] Slice 5 — range patterns (with #990)
- [ ] Slice 6 — lambda-parameter destructuring (conditional)
- [ ] Slice 7 — tuple patterns (conditional)

## 10. Non-goals

- **Closed record patterns** (Q1 (b)), and any form in which adding a field can
  break a pattern.
- **Destructuring in function signatures** (Q5).
- **`Map` and `Set` patterns.** Their identity is by key, not position.
- **As-patterns** (`n @ pattern`). Useful, but a separate binding form; a later
  proposal if wanted.
- **View patterns or user-defined extractors.**

## 11. Done when

- Every slice in §9 has shipped or been explicitly dropped with a recorded
  reason.
- The spec states record, list and range patterns, the `..` rules and the
  exhaustiveness algorithm normatively, in spec chapters 4 and 5.
- Every existing `match` emits byte-identically, proved by the existing goldens.
- A fixture shows that adding a field to a record leaves every record pattern on
  it compiling.
- The retirement PR removes this doc, appends its closing summary to
  `archive/retired-tracks.md`, and closes #2616.
