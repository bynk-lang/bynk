# Increment proposals

A proposal is a **transient input** to an increment, not a durable artefact: the
durable record of a landed increment is the code and its fixtures, the
[normative spec](../../site/src/content/docs/book/spec/index.md) updated in place, the
[decision records](../decisions/README.md), and the **reader-facing book**
(`site/src/content/docs/book/` — guides, reference, tutorials, changelog, roadmap) left current
and reliable. The spec is normative; the book is what a newcomer or evaluator
actually reads, and it is part of the increment, not a follow-up.

Because a proposal is transient and discussion-shaped, its home is a **GitHub
issue**, not a committed-then-deleted file. Open one with the
[increment-proposal template](../../.github/ISSUE_TEMPLATE/increment-proposal.md)
and label it `proposal`. This directory is only for **long-form drafts under
active refinement** that benefit from line-by-line diffs before promotion (see
[Drafting](#drafting-long-form-proposals)); such a draft is named `draft-<slug>.md`,
is **not** committed, and is deleted once promoted to an issue.

## Lifecycle

1. **Propose.** Open an issue from the
   [increment-proposal template](../../.github/ISSUE_TEMPLATE/increment-proposal.md)
   (label `proposal`). A proposal that slices a [feature track](../tracks/README.md)
   is opened as a **sub-issue of the track's spine issue**, citing the track doc
   and its foundational ADRs — the spine's sub-issue checklist is the track's
   live slice status (ADR 0167). It is the sign-off artefact: the design forks marked
   `[DECISION]` with recommendations, the risks, a sketch of the spec delta, and
   a **docs delta** — which book pages the increment adds or changes (see
   [Documentation is part of done](#documentation-is-part-of-done)). A proposal
   with no docs delta must say *why* the book is untouched; silence is treated as
   an oversight, not a no-op. Do **not** pre-allocate the increment's version or
   ADR number — both are assigned at implementation (cite only *shipped*
   versions/ADRs as provenance). **Accepting the issue (label `accepted`) is the
   approval to build.**
2. **Implement.** The increment consumes the proposal: the grammar/compiler
   change with fixtures, the spec chapters updated in place, a decision record
   per language-defining call, the **book and changelog deltas** that keep
   `site/src/content/docs/book/` current and reliable, **the tooling deltas —
   explicitly naming hover, completion, semantic tokens, and signature help**
   (a construct a language slice adds or changes must say what each of the
   four does now, even if the answer is "unchanged, because …"; silence is
   treated as an oversight, the same rule the docs delta above holds itself
   to — see [ADR 0156](../decisions/0156-editor-surface-tracks-language.md)),
   and — for a language/tooling increment — the version bump
   (`scripts/bump-version.sh` — see
   [Versioning & release](../README.md#versioning--release)). The book delta
   is a completion criterion, not optional polish. A docs-only increment carries
   **no version bump**: there is no language or tooling artefact to version, and
   the book's own currency banner is advanced to whatever version it now
   describes, not to a new one of its own.
3. **Close.** The implementing PR closes the proposal issue with `Closes #<n>` —
   no manual delete step, and the issue↔PR↔commit links are automatic. The
   issue's full history stays on the tracker (closed, not deleted), so an ADR can
   cite `proposed in #<n>` as **durable plain-text provenance** — an issue number
   resolves forever, unlike a proposal file path. On merge, a language/tooling
   increment is tagged `vX.Y.Z`; a docs-only increment is **not tagged** — it
   ships with its PR and bumps nothing.

## Drafting long-form proposals

A short proposal is written issue-native from the start. A large,
`[DECISION]`-heavy one benefits from line-anchored review and revision diffs that
an issue body does not give — draft it here as `draft-<slug>.md` (untracked),
iterate, then **promote the reviewed result to an issue** and delete the draft.
Drafting and the transient-tracking role need not use the same medium.

## Writing one

Write a proposal knowing it will be consumed, not maintained. State **deltas
and decisions** — what changes, the forks and their recommendations, what the
spec sections will say. Do **not** duplicate normative content (full grammar
productions, worked emission output): the normative prose is written once, in
the spec, during implementation. Duplicated content is how the retired
instalment documents drifted.

## Documentation is part of done

The spec proves the language is *defined*; the book is what makes it *usable and
trusted*. An increment that ships a feature but leaves the book stale has not
landed cleanly — to a newcomer or evaluator a self-contradicting book reads as
"not seriously maintained", which is the most expensive impression a pre-1.0
language can give. So every increment leaves `site/src/content/docs/book/` **current and reliable**.
Concretely, the implementing PR must, *where the increment touches them*:

- **Document new or changed surface in the book, not only the spec.** A new
  language/tooling feature gets its reference page **and** its guide entry — and,
  for a genuinely new *concept*, the "Understand" on-ramp, not just a "Do"
  recipe. The spec is for lookup; the guides are the learning path.
- **Keep currency claims true.** Advance the "written against vX.Y" banner and
  `spec/appendix-version-history.md`; add the `reference/changelog.md` entry. The
  version the book *claims* to describe and the version it *does* describe must
  match.
- **Keep the roadmap honest in both directions.** Shipped behaviour must never be
  listed as "next/planned", and aspirational design must never be written in the
  present tense the book reserves for "what compiles today". Prefer naming
  *intent* over version-pinned milestones that rot on the next release.
- **Leave no dead links or stale framing.** Moving/renaming pages updates every
  inbound link (including the repo `README.md`) and any prose describing the docs'
  shape. The link-check / doc-example / spelling gates catch broken *references*;
  they do **not** catch a sample corrupted inside a code fence or a claim that
  silently went out of date — those are the author's responsibility.
- **Ship an explanation with each new error code.** A new *error* diagnostic
  code gets a curated `bynk explain` entry (`EXPLANATIONS` in
  `bynk-syntax/src/diagnostics.rs`): why the rule exists and a before-and-after
  example. The example is backed by a compiled pair,
  `bynk-tests/tests/fixtures/explain/<code>/bad.bynk` and `good.bynk`: the first
  must fail with that code alone, reported on a line the example marks `✗`,
  the second must compile, and each line of the example must appear in one of
  them (`explain_examples.rs`). Warnings and
  internal codes may be left to the one-line registry summary. The `explained_codes` probe ratchets coverage
  upwards; the backlog is tracked in #3163.
- **Keep examples consistent and compiling.** Snippets demonstrating the same
  thing use the same form; prefer `{{#include}}` from a compiled worked example
  over hand-written fences so CI keeps them honest and they cannot rot.

If the increment genuinely touches none of these (e.g. a pure internal refactor),
say so in the proposal's docs delta — an explicit "no book impact, because …"
rather than an absent section.
