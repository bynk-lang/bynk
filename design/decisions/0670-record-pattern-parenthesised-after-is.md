# 0670 — A record pattern after `is` is parenthesised

- **Status:** Accepted (v0.455)

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
