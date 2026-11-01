---
level: minor
changelog: "Record destructuring in `let`: `let Item { sku, qty, .. } = item` binds a record's fields for the rest of the block, with `=` or `<-`, and needs no `else` when every field is matched by a name, `_` or a nested pattern of the same kind; a refutable record pattern still takes its `else` (patterns track slice 2, [#2654](https://github.com/bynk-lang/bynk/issues/2654)). A record `let … else` that cannot fail now offers a one-click fix that removes its `else`"
---

## ADR: record-destructuring-in-let
title: A `let` destructures a record, and omits `else` when the pattern cannot fail
summary: `let T { f, .. } = e` opens a record pattern in `let` (a capitalised name followed by `{`), and a `let` whose pattern is an irrefutable record pattern needs no `else`; it reuses the ADR 0484 one-arm `match` desugaring and adds no type rule; realises patterns-track slice 2

**Context.** Slice 1 added record patterns to `match` and `is`, but a record
still could not be destructured by `let`: `let_pattern_ahead` opens a pattern
only for a literal, `(`, `Ok`/`Err`/`Some`/`None`, or a name followed by `(` or
`.`. The parenthesised workaround, `let (Item { sku, .. }) = item else { … }`,
parsed but was rejected because its `else` can never run
(`bynk.types.unreachable_arm`), and a plain `let` holds no pattern. Reading three
fields therefore took three `let x = r.x` lines. The patterns track
(`design/tracks/patterns.md` §5 Q5) settled that `let` destructuring is slice 2.

**Decision.**

- **A. The `let` opener.** A capitalised name followed by `{` opens a pattern
  after `let`, in both parsers. It is unambiguous: a binding name is only ever
  followed by `:`, `=` or `<-`, so `let Name {` was a parse error in both parsers
  before this change and no program that parsed changes meaning. The spike that
  confirmed this ran first (see the PR).
- **B. Irrefutability is judged in the parser.** A pattern `let` without an
  `else` is accepted when its top-level pattern (parentheses are transparent) is a
  record pattern and `Pattern::is_irrefutable` holds: every field is a shorthand
  binding, `f: name`, `f: _`, or a nested irrefutable record pattern. Any other
  pattern without an `else` is still `bynk.parse.let_pattern_without_else`. For
  a record pattern the diagnostic names the first field that can fail to match.
  A bare uppercase name is a variant, so irrefutability is syntactic.
- **C. Only record patterns gain the `else`-less form.** `let (x) = e` is just
  `let x = e`, and a single-variant sum or an or-pattern of irrefutable
  alternatives stays `else`-only. The rule is one sentence: a `let` may omit
  `else` when its pattern is an irrefutable record pattern.
- **D. Lowering reuses ADR 0484.** `let P = e` desugars to the hidden binder and
  a one-arm `match binder { P => { rest } }`, recorded as the new
  `LetElseForm::Plain` so the formatter prints no `else`. Slice 1's conservative
  coverage counts an all-irrefutable record arm as covering, so no checker rule is
  added; scoping, shadowing, type mismatch, unknown and duplicate fields, generic
  arguments and `bynk.context.opaque_inspection` are slice 1's and ADR 0484's
  unchanged. Emission goes through the existing if-chain, and the corpus emits
  byte-identically.
- **E. An irrefutable record `let … else` keeps `bynk.types.unreachable_arm`,**
  now with a machine-applicable "remove the `else`" fix. The fix deletes the
  `else` clause; parentheses the author wrote around the pattern are left, since
  they are valid and the AST does not retain them. Other patterns are unchanged.
- **F. The parenthesised form is accepted:** `let (T { … }) = e` is the same
  pattern, as `paren_pattern` is transparent everywhere.
- **G. `<-` is supported,** with the optional call-site actor clause, as for
  `let … else`.

**Consequences.**

- Destructuring a record is one `let`, and adding a field to the record breaks
  none of them, since every record pattern is open.
- A refutable record pattern without an `else` is a parse error that names the
  field, never a silent runtime failure.
- The tree-sitter grammar cannot tell a binding from a variant (capitalisation
  is not visible to it), so it admits the `else`-less form for any record
  pattern; the compiler alone rejects a refutable one. The cross-parser
  conformance test pins the agreement everywhere else, and the one negative
  fixture that depends on it is excluded by name from the totality check.
- Lambda-parameter destructuring (slice 6) and precise exhaustiveness (slice 3)
  are untouched.
