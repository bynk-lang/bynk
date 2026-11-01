---
level: minor
probes: emit_diagnostics, ts_writes, diagnostic_coverage
changelog: "Integer range patterns: `lo..hi`, `lo..` and `..hi` (inclusive, bounds optionally negative) match an `Int` in a `match`, nested in variant, record and list patterns, in or-patterns and in `let … else`. Exhaustiveness is by interval, so `..-1` and `0..` need no `_`, and a missing interval is reported as a pattern (`13..17`). `via schema(...)` takes the same ranges and `_` (events slice 4b, [#990](https://github.com/bynk-lang/bynk/issues/990)), through one shared production. `[..9]` in a list pattern is now a range element, which retires `bynk.parse.range_pattern_unsupported`. New diagnostics: `bynk.types.range_pattern_on_non_int`, `bynk.types.empty_range_pattern` and `bynk.types.is_range_pattern` (patterns track slice 5, [#2699](https://github.com/bynk-lang/bynk/issues/2699))"
---

## ADR: range-patterns
title: One integer-range production serves match patterns and `via schema(...)`, and `..` is a rest marker or a range bound by the next token
summary: Inclusive integer ranges `lo..hi`, `lo..`, `..hi` as one `int_range` production shared by `match` patterns and `via schema(...)`; Int-only; empty ranges rejected; interval constructors split at the column's endpoints in the usefulness engine; comparison lowering in the if-chain; the rule that decides `..` between a list rest marker and a range, closing slice 4's reservation; realises patterns-track slice 5 and events slice 4b

**Context.** An `Int` was matched by literal, or by `_ where InRange(lo, hi)`,
which binds nothing and which the coverage engine treats as matching nothing, so
neither a band of numbers nor an overlap between bands could be checked. Events
slice 4b (#990) needed the same notion for `via schema(2..)`, and the patterns
track required the two to share a production so the language does not grow two
range forms. Slice 4 (ADR 0685) reserved `..` followed by a literal as
`bynk.parse.range_pattern_unsupported` for this slice.

**Decision.**

- **A. `..` cannot be ambiguous (spike, recorded before building).** Checked
  against both parsers and the lexers: `IntLit DotDot`, `DotDot IntLit` and
  `DotDot Minus IntLit` began no pattern, arm or `let` pattern before this slice
  outside the reservation (probes: `0..9 =>`, `let ..9 =`, `x is 13..17` all
  failed at the `..`); `0..9`, `18..`, `..-1` and `1_000..2_000` lex as integer,
  `..`, integer tokens in the hand-written lexer, because a float needs a digit on
  both sides of its `.`, and as `number_literal ".." number_literal` in
  tree-sitter; and `tree-sitter generate` on the new rule reported no conflict in
  a pattern position. The one conflict it did report is in the `is` position and
  is resolved under C.
- **B. One production, one parser function.** `int_range` is `lo..hi`, `lo..`
  or `..hi`, each bound an optionally negative `Int` literal; a bare `..` is never
  a range. `Parser::int_range_ahead` and `Parser::parse_int_range` are the only
  recognition and parse, used by `parse_pattern_base`, `let_pattern_ahead`, a list
  pattern's elements and `parse_schema_dispatch_clause`. The AST is
  `Pattern::Range(IntRange { lo, hi, span })` and
  `SchemaVersionPattern::{Literal, Range(IntRange), Wildcard}`.
- **Q7's rule: the next token decides.** After `..`, an integer (or `-` and an
  integer) makes the `..` a range bound; a lowercase name, `,` before `]`, `]` or
  nothing makes it the rest marker. The two never overlap, since a rest marker is
  never followed by a number. Slice 4's reservation is retired and `[..9]` is a
  range element; `..` followed by a string or boolean is a plain
  `bynk.parse.expected_token`. The high bound of `lo..` is read greedily
  (`5.. -1` is `5..-1`), which is safe because a pattern is always followed by
  `=>`, `if`, `|`, `,` or a closing delimiter.
- **C. `is` takes no range, and tree-sitter does not parse one there.** A range
  directly after `is` parses in the compiler and is
  `bynk.types.is_range_pattern`, steering to `match`, a guard or `InRange`, as a
  literal is `bynk.types.is_literal_pattern`. The tree-sitter grammar leaves
  `int_range` out of `_is_base_pattern`: an arm body ending `x is 5` followed by an
  arm starting `..9` would otherwise be one range, which is an LR conflict the
  grammar cannot resolve without newlines. The compiler is unaffected, and
  `is` with a range is an error either way, so the editor only loses a
  highlight on a rejected program.
- **D. Only `Int`.** Any other scrutinee is `bynk.types.range_pattern_on_non_int`,
  which notes that `Float` (NaN, representation) and `String` are deliberate
  non-goals. A refined `Int` is matched as `Int`; its own bounds are not used,
  following slice 3. Bounds beyond the safe-integer range are the lexer's
  integer-literal error.
- **E. Malformed ranges are errors.** `lo > hi` is
  `bynk.types.empty_range_pattern`, with a `MaybeIncorrect` fix that swaps the
  bounds; `via schema(...)` reuses the same check. `lo..lo` is the literal.
- **F. Coverage by interval, in the existing engine.** `Constructor::IntRange(lo,
  hi)` is inclusive and an `Int` literal lowers to the singleton `IntRange(v,
  v)`. `Space::Int` covers `-(2^53 − 1)` to `2^53 − 1` and splits a column at
  every endpoint its patterns use (a cut at each low bound and just after each
  high bound), so each pattern covers whole intervals and the split is linear in
  the number of endpoints. A query range splits along the same cuts, so an arm
  wholly covered by earlier arms is `unreachable_arm` and a partly covered one is
  legal. Uncovered intervals render as `lo..hi`, `lo..`, `..hi` or the single
  value, and adjacent uncovered intervals with the same other columns merge into
  one witness. `STEP_BUDGET` and `MAX_WITNESSES` apply unchanged.
- **G. A column of single values is not split.** `Space::Int` offers constructors
  only when some pattern in the column is not a single value. A match of `Int`
  literals alone keeps the witness `_` and the existing message, so no existing
  program's diagnostics, and no existing fill-arm suggestion, change.
- **H. Lowering is comparisons in the if-chain.** A range tests `x >= lo && x <=
  hi`, one side only for an open range, and forces the if-chain
  (`match_needs_if_chain`); flat literal `switch` matches and every existing
  golden are byte-identical. `via schema(...)` guards are `===` for a literal,
  `>=`/`<=` for a range and none for `_`. `_` still triggers the synthetic
  envelope parameter, so a sibling relying on it is unaffected.
- **I. `via schema(...)` follows #990 as written.** Inclusive bounds, `_`, each
  bound a positive `Int` (`bynk.event.bad_schema_dispatch`), `lo > hi` rejected,
  and no sibling-overlap diagnostic: delivery stays deliver-and-filter. #990's
  `OpenAbove`/`OpenBelow`/`Closed` variants are the single `Range(IntRange)` here,
  with identical semantics. In the IR the field widens from `Option<i64>` to
  `Option<SchemaVersionIr>`.

**Consequences.**

- An `Int` match can be exhaustive with no `_` (`..-1` and `0..`), and the
  compiler names the interval a match leaves out.
- `_ where InRange(lo, hi)` is unchanged and still matches nothing in coverage;
  ranges are the checkable form.
- Not available: `Float`, `String`, `Duration` and `Instant` ranges, exclusive
  bounds, named-constant bounds, and as-patterns (`n @ 0..9`). Each is additive.
- The compiler and tree-sitter disagree on one rejected program (a range after
  `is`), recorded under C.
- A module whose every `via schema(...)` subscriber omits `env` imports no
  `EventEnvelope` for its synthetic envelope parameter, a gap that predates this
  change and that the behavioural test works around as slice 4's does.
