---
name: Increment proposal
about: Propose a language / compiler / emitter / tooling increment. The accepted issue is the approval to build; it replaces the old design/proposals/*.md file.
title: "<one-line increment title — no version prefix; the version is assigned at implementation, not here>"
labels: proposal
---

<!--
A proposal is a TRANSIENT input to an increment, not a durable artefact. The
durable records are the code + fixtures, the normative spec updated in place, the
decision record (ADR), and the reader-facing book left current. See
design/proposals/README.md.

Do NOT pre-allocate this increment's version or ADR number: the version is set by
scripts/bump-version.sh when the implementing PR lands, and the ADR number is the
next free one taken at merge. Cite only SHIPPED versions/ADRs as provenance.

The implementing PR closes this issue with `Closes #<this>` — there is no manual
file-delete step.

Every section below is required. Where a section genuinely does not apply, say so
explicitly with a reason ("none, because …"). Silence is treated as an oversight,
not a no-op — the same rule the docs and tooling deltas hold themselves to.
-->

## Summary

- **Scope:** <what layers change — grammar / AST / checker / emitter / runtime / tooling — and what stays untouched.>
- **Addresses:** <the concrete gap or defect today, grounded in file:line where possible. Link the issue(s) that surfaced it.>
- **Realises:** <the end state in one or two sentences — what a correct version does.>

## Framing (why this is the language's to fix)

<Optional but encouraged for anything non-obvious: why this belongs in the compiler/runtime rather than being hand-rolled by the author.>

## What exists today (grounded)

<The relevant current behaviour, cited to file:line. This is what the increment consumes.>

## The surface

<The author-facing surface, if any. If there is none — a pure wire/behaviour or internal change — say so and describe the observable behaviour instead.>

## Decisions

<!-- Each fork is a [DECISION X] block with a recommendation. These are the sign-off points. -->

**[DECISION A] <the fork> (Recommended: <option>).** <the reasoning and the consequence stated plainly.>

## The deltas (concretely)

- **Grammar / AST (`bynk-syntax`).** <changes, or "None, because …">
- **Checker (`bynk-check`).** <changes, or "None, because …">
- **Emitter (`bynk-emit`).** <changes.>
- **Runtime.** <changes, or "None.">

## Risks & mitigations

- <risk> → <mitigation>

## Docs delta

<!-- Documentation is part of done. A proposal with no docs delta must say WHY the
book is untouched; silence is an oversight. See design/proposals/README.md §"Documentation is part of done". -->

- **Reference / Guide / Spec:** <which book pages the increment adds or changes; a genuinely new *concept* earns an "Understand" on-ramp, a refinement earns a reference/recipe update.>
- **Changelog + version history:** <advance the currency banner and appendix-version-history to the version this increment ships as (not a pre-assigned one); changelog entry.>
- **Roadmap:** <kept honest in both directions.>

## Tooling delta (ADR 0156 — silence is an oversight)

<!-- A language/tooling increment MUST say what each of the four does now, even if the answer is "unchanged, because …". -->

- **Hover:** <changed / unchanged, because …>
- **Completion:** <changed / unchanged, because …>
- **Semantic tokens:** <changed / unchanged, because …>
- **Signature help:** <changed / unchanged, because …>

## Done when

- <observable acceptance criteria — the behaviour a reviewer can check.>
- Fixtures (next free indices) cover <the cases>. A change to what a program *does* at runtime is proved by a behavioural fixture (an `expected_run.txt` suite run by `bynk-tests/tests/behaviour_fixtures.rs`), not only by a golden.
- Docs current per the delta above; tooling surfaces stated.
- Version bump (`scripts/bump-version.sh`) for a language/tooling increment.
- A new ADR records the forks; its number is assigned when the implementing PR lands. The implementing PR closes this issue (`Closes #<this>`).
