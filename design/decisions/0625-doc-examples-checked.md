# 0625 — Doc-comment examples are type-checked by `bynk test`

- **Status:** Accepted (v0.414)

**Context.** Fenced `bynk` examples in doc comments were never compiled, so they
could silently fall out of date with the code they document. The Book's own
examples were already gated (`bynkc/tests/doc_examples.rs`); a project's own were not.

**Decision.** `bynk test` type-checks every fenced `bynk` block in a `---`
doc-block of a `commons` or `context`. A block of statements is a `case` in a
synthetic `suite` that targets the declaring unit, so the example has the unit's
scope and the existing test semantics. A block of declarations is a synthetic
sibling `commons` that `uses` the declaring unit, when that is a `commons`; in a
`context` it is not checked, because a sibling can't reach the context's own
types and a copy of the context collides with the declaration the doc shows, so
either mode would report false errors. A
block that parses as neither is a parse error at the fence; the choice is made
by parsing the body. The check is a project check after the real project compiles
(an example is never blamed for an error elsewhere), and a diagnostic is
mapped back to the line inside the fence. Examples are checked, not run, by default. The
info string's first word is `bynk`; later words, separated by spaces or commas,
select `ignore`, `fragment` or `fail` to skip the block. Any other fence is not
looked at. Running examples marked as tests, and the test explorer grouping,
are later slices.

**Consequences.** A project with a stale example now fails `bynk test`; the
remedy is to fix it or mark it `bynk ignore`. There is no language or grammar change and no emitted output
change. Checking costs one extra project check, and only when a project has
such a fence.
