# Contributing to Bynk

Thanks for helping. By taking part you agree to the
[Code of Conduct](https://github.com/bynk-lang/.github/blob/main/CODE_OF_CONDUCT.md).
To report a security issue, follow [SECURITY.md](SECURITY.md), not the issue
tracker.

Bynk is pre-1.0 and its language is designed deliberately, so the route a change
takes depends on what it changes.

## Which route?

| Your change | Route |
| ----------- | ----- |
| A typo, a docs fix, an obvious bug fix | Open a pull request directly. |
| A bug you are not going to fix yourself | Open a [bug report](https://github.com/bynk-lang/bynk/issues/new?template=bug_report.yml). A [playground](https://playground.bynk-lang.dev) link to the smallest program that shows it helps; in VS Code, **Bynk: Copy Playground Link** makes one from a selection. |
| Anything that changes the language, its tooling surface, or the standard library | Open an **increment proposal** first. Do not start with a PR. |
| A large feature spanning several increments | A **feature track**; talk to the maintainers first. |

### Increment proposals

A proposal is a GitHub issue, opened from the
[increment-proposal template](.github/ISSUE_TEMPLATE/increment-proposal.md) and
labelled `proposal`. It is the sign-off artefact: it lays out the design forks
with recommendations, the risks, a sketch of the spec delta, and a docs delta.
Accepting it (label `accepted`) is the approval to build. Please wait for that
before writing the implementation.

The full lifecycle, and what an implementing PR must leave behind (spec, book,
decision records, editor-tooling deltas), is in
[`design/proposals/README.md`](design/proposals/README.md). Feature tracks are
described in [`design/tracks/README.md`](design/tracks/README.md).

### Design decisions

Language-defining calls are recorded as decision records in
[`design/decisions/`](design/decisions/README.md). A feature PR does not choose
its own version or ADR number: it adds one `design/pending/<slug>.md` carrying
intent only, and a merge-time stamp assigns the numbers in merge order. See
[`design/pending/README.md`](design/pending/README.md).

## Developing

Build and layout are described in the [README](README.md#developing), and the
compiler's architecture, its test fixtures and how the docs are built in the
[contributor pages](https://bynk-lang.org/project/contributing/). Before you
push, run the CI gates locally:

```sh
cargo xtask ci          # formatting, clippy, strict rustdoc, and the test suite
cargo xtask ci --fast   # formatting, clippy and strict rustdoc; no tests
```

An opt-in `pre-push` hook runs the fast form; enable it once per clone:

```sh
git config core.hooksPath .githooks
```

CI remains the authority. The toolchain is pinned in `rust-toolchain.toml`.

**Ordered reference lists.** The long lists in the Reference (the type tables and
sections of `reference/types.md`, the terms of `reference/glossary.md`) are kept in
alphabetical order. A list that is ordered is wrapped in a marker, so the check never
guesses: `<!-- sorted: table -->` around a table (its rows are ordered by their first
code span) or `<!-- sorted: headings -->` around sections (ordered by heading), each
closed by `<!-- /sorted -->`. When you add an entry to a marked list, put it in its
place, and move whole entries only: a table row, or a heading with its section.
`cargo xtask sorted-lists` (part of `cargo xtask ci --fast`) names any entry that is
out of order and the one it should follow. Keys ignore case, backticks, symbols and a
leading "The".

## Pull requests

1. Branch from `main`. Keep the PR to one concern.
2. Open it as a **draft** while you work; mark it ready when it is. Fill in the
   pull request template.
3. Link the issue: `Closes #<n>` for the proposal or bug it implements.
4. Add tests and fixtures with the change. A language or tooling change also
   updates the normative spec and the book (`site/src/content/docs/book/`) in the
   same PR: documentation is part of done.
5. The required check is `CI green`. `main` is protected and merges go through
   the merge queue, so you do not merge by hand and do not need to rebase just to
   stay current.
6. CI runs in stages (the table at the top of `.github/workflows/ci.yml`):
   - **Draft:** each push runs the fast Linux checks only, and `CI green` stays
     red, because the full check has not run.
   - **Ready for review:** adds the Windows leg (and macOS, when the
     repository variable `MACOS_ON_PRS` is `true`), the Linux system tests,
     TypeScript 7 and the Node floor, so platform and system-test failures
     show up while the PR is in review. A same-repo draft is marked
     ready automatically once its fast checks pass (add the `keep-draft` label
     to stop that).
   - **Merge queue:** re-runs the fast Linux checks on top of the PRs queued
     ahead, to catch changes that only break in combination.
   - **Nightly:** covers the rest, and opens a `Nightly CI failed` issue when it
     goes red.

## Licence

Licensed under either of Apache-2.0 ([LICENSE-APACHE](LICENSE-APACHE)) or MIT
([LICENSE-MIT](LICENSE-MIT)), at your option, except the documentation's prose
(the pages in `site/src/content/`), which is [CC BY 4.0](LICENSE-CC-BY-4.0.md),
and its code snippets (code blocks in those pages, and `site/src/snippets/`),
which are [MIT](LICENSE-MIT). Unless you explicitly state otherwise, any
contribution intentionally submitted for inclusion in the work by you, as
defined in the Apache-2.0 licence, shall be licensed in the same way, without
any additional terms or conditions. The README's "Licence" section has the
details.
