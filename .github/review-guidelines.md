# Review guidelines for Bynk

These are the maintainers' notes for reviewing a pull request here. This is
the Bynk language: the compiler and its crates, the standard library, the
language server and tooling, the tree-sitter grammar, and the docs site
(`site/`), with the design records in `design/`. Bynk is pre-1.0 and designed
deliberately, so a change is judged against the decisions already recorded
as well as on its own.

Skip what CI already enforces: formatting, clippy, strict rustdoc, the test
suite on every platform, the compiled doc examples, the docs build and its
link checker, and the stamp's checks on `design/pending/` files. A failing
check speaks for itself.

## How to work

Work from the diff, and open a file only when a finding needs it. Don't browse
`design/` or the tree broadly: read the one ADR, spec section or README a
finding depends on. Most pull requests here are large, so spend the review on
what could be wrong, not on describing the change.

## What matters most

- **Correctness.** Logic errors, edge cases and boundary conditions the
  changed code could get wrong. In the compiler, think about the programs the
  change wasn't written for: nested and generic forms, the error paths, empty
  and maximal inputs, and every target and platform the code emits for.
- **Safety.** Code Bynk generates runs in other people's services, so a
  change to what the compiler emits must keep its guarantees: validation at
  the boundary, effects only through declared capabilities, secrets never in
  output or logs. Flag a change that weakens one, and any `unsafe`, `unwrap`
  or panic reachable from user input in the compiler or the language server.
- **Tests at the edge.** A change comes with tests and fixtures that exercise
  where it could be wrong, not just the happy path. A new diagnostic has a
  fixture that triggers it; a language change has passing and failing
  programs.
- **Decided before built.** A change to the language, its tooling surface or
  the standard library follows an accepted increment proposal (an issue
  labelled `proposal` and `accepted`), and the pull request links it. Flag one
  that changes those surfaces with no accepted proposal, or that departs from
  what the proposal or an existing ADR in `design/decisions/` settled.

## Release discipline

- **One pending file.** A non-trivial code change carries exactly one
  `design/pending/<slug>.md`, with a `level` (`minor` for a language increment,
  `patch` otherwise) and a one-line Markdown `changelog`. Flag one that's
  missing, a second one, or one added to a pure docs or infrastructure change.
  See `design/pending/README.md` only if you need to confirm the rules.
- **No numbers.** The pull request never picks a version or an ADR number: the
  merge-time stamp assigns both, and writes the changelog row on the release
  pages (`site/src/content/docs/reference/releases/`) from the pending file.
  Flag a hand-edited version, ADR number or changelog row. `CHANGELOG.md` at
  the root is no longer maintained.
- **Probes.** A pull request with a pending file names the gated greenfield
  probes it moves in that file's `probes` key, rather than editing
  `design/greenfield-status.md`, whose table the stamp rewrites on merge. Flag
  a hand-edited table alongside a pending file, and trend rows changed by a
  feature pull request.
- **Documentation is part of done.** A language or tooling change updates the
  normative spec and the Book (`site/src/content/docs/book/`) in the same pull
  request. A change that retires, renames or changes a user-facing surface
  also updates the README and the about pages. Flag a missing doc delta.

## Style

- British English in comments, docs, diagnostics and every message a user
  reads ("behaviour", "initialise", "licence" as a noun).
- A diagnostic says what went wrong and what to do about it, in plain words,
  with its code.
- Comments explain why, and name the issue or ADR behind a decision.

## Severity here

- **Blocking:** a correctness bug a user's program could hit, a weakened
  safety guarantee in emitted code, a panic reachable from user input, a
  language change with no accepted proposal, or a version or ADR number
  chosen by hand.
- **Should fix:** a missing test at the edge the change could get wrong, a
  missing or extra pending file, a hand-edited greenfield table, a missing doc
  delta, or an unclear diagnostic.
- **Nit:** wording, naming and small style points. Keep these few. If the
  change is sound, say so briefly in the summary.
