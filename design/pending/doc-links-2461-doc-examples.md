---
level: minor
changelog: "`bynk test` type-checks the fenced `bynk` examples in doc comments, reporting a mistake at the example's line; `bynk ignore` opts out"
---

## ADR: doc-examples-checked
title: Doc-comment examples are type-checked by `bynk test`
summary: A fenced `bynk` block in a doc-block is checked as a synthetic `case` on its unit, not run

**Context.** Fenced `bynk` examples in doc comments were never compiled, so they
could silently fall out of date with the code they document. The Book's own
examples were already gated (`bynkc/tests/doc_examples.rs`); a project's own were not.

**Decision.** `bynk test` type-checks every fenced `bynk` block in a `---`
doc-block of a `commons` or `context` as a `case` in a synthetic `suite` that
targets the declaring unit, so the example has the unit's scope and the existing
test semantics. The check is a project check after the real project compiles
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
