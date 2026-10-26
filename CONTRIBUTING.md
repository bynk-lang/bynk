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
| A bug you are not going to fix yourself | Open a [bug report](https://github.com/bynk-lang/bynk/issues/new?template=bug_report.yml). |
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

Build and layout are described in the [README](README.md#developing). Before you
push, run the CI gates locally:

```sh
cargo xtask ci          # formatting, clippy, and the test suite
cargo xtask ci --fast   # just formatting and clippy
```

An opt-in `pre-push` hook runs the fast form; enable it once per clone:

```sh
git config core.hooksPath .githooks
```

CI remains the authority. The toolchain is pinned in `rust-toolchain.toml`.

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

## Licence

Licensed under either of Apache-2.0 ([LICENSE-APACHE](LICENSE-APACHE)) or MIT
([LICENSE-MIT](LICENSE-MIT)), at your option. Unless you explicitly state
otherwise, any contribution intentionally submitted for inclusion in the work by
you, as defined in the Apache-2.0 license, shall be dual licensed as above,
without any additional terms or conditions.
