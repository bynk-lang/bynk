# Design notes

Internal design records for Bynk. These are the working references behind the
language and tooling — distinct from the published book under `docs/`, which is
the canonical, reader-facing spec and reference.

## Current state

**Strategy** — the load-bearing calls, decided once:

- [`bynk-positioning.md`](bynk-positioning.md) — the **identity decision** (#540
  §7(1)): Bynk is a production service-tier language; pedagogy is kept as a
  constraint (it *teaches as it is used*), and the research-instrument framing is
  dropped. Read this before the design notes — it governs how their pedagogy
  framing is meant.
- [`bynk-adoption-sequencing.md`](bynk-adoption-sequencing.md) — the **roadmap
  sequencing decision** (#540 §7(2)): the three adoption blockers — deploy →
  migrations → ecosystem posture — come first, ahead of the v1 language-vision
  tracks; tooling *depth* is frozen (currency is not). Governs the roadmap
  ordering in the status doc §6.
- [`bynk-1.0-definition.md`](bynk-1.0-definition.md) — the **1.0 definition**
  (#540 §7(4)): 1.0 = Foundations-layer stability + `deploy` + state migrations —
  a stability commitment, not a feature count; events/sagas are post-1.0
  additive. Governs what "1.0" means and what it defers.
- [`bynk-release-discipline.md`](bynk-release-discipline.md) — the **release
  discipline** (#540 §7(4)): daily increments batch into releases, cut at each
  track retirement before 1.0 and as named **monthly milestones** from 1.0,
  with cumulative migration notes (and post-1.0 codemods), and
  **doc-truth drift guards** keep the README/about pages honest. The mechanism
  that keeps the 1.0 promise legible and verifiable.

**Status & roadmaps** — where the project is and where it's going:

- [`bynk-status-and-roadmap.md`](bynk-status-and-roadmap.md) — the **status & gap
  audit** (refreshed per release; currently v0.54). Start here. The root
  `README.md` links to it.
- [`bynk-tooling-roadmap.md`](bynk-tooling-roadmap.md) — the editor-experience
  forward plan (LSP + VS Code), including the remaining tooling backlog.
- [`vscode-extension-extraction.md`](vscode-extension-extraction.md) — how the VS Code
  extension was extracted into `bynk-lang/vscode-bynk`: couplings, history
  split, contract and CI replacements, and what execution changed.
- [`bynk-engineering-roadmap.md`](bynk-engineering-roadmap.md) — the CI/CD
  pipeline plan and the `bynkc` internal-quality refactor backlog.
- [`bynk-compiler-trajectory.md`](bynk-compiler-trajectory.md) — the **compiler
  architecture theme**: eight phases from the shipped compiler to
  `bynk-greenfield-compiler.md`, each a safe stopping point, realised one track
  per phase (`tracks/compiler-architecture.md` is the first, covering phases
  0–2). **Complete** — all eight phases retired, the last (incrementality) on
  30 August 2026; the document stays in place as the historical record.

**Canonical design** — the long-form rationale and the type theory:

- [`bynk-design-notes.md`](bynk-design-notes.md) — the long-form design rationale
  (the aspirational v1 language).
- [`bynk-type-system.md`](bynk-type-system.md) — the type system in depth
  (aspirational; carries an implementation-status banner).
- [`bynk-greenfield-compiler.md`](bynk-greenfield-compiler.md) — the compiler
  rebuilt on today's architectural understanding, feature for feature
  (aspirational; 130 numbered rules); the endpoint
  `bynk-compiler-trajectory.md` migrates the shipped compiler toward.

**Tooling specs** — capability contracts, referenced from code:

- [`bynk-lsp-spec.md`](bynk-lsp-spec.md) — LSP capabilities; referenced from
  `bynk-lsp/src/main.rs`, `bynkc/src/fmt.rs`, and ~18 ADRs.
- [`bynk-tree-sitter-spec.md`](bynk-tree-sitter-spec.md) — tree-sitter highlight
  groups; referenced from `tree-sitter-bynk/queries/highlights.scm`.

**Process directories:**

- [`proposals/`](proposals/README.md) — **increment proposals**: the transient
  sign-off artefact for an increment, living as a **GitHub issue** (label
  `proposal`; `accepted` = approval to build) and closed by the PR that
  implements it. The directory holds only long-form untracked drafts.
- [`tracks/`](tracks/README.md) — **feature-track design docs** (ADR 0076, run
  GitHub-native per ADR 0167): the *persistent* design + slice decomposition for
  a far-reaching, multi-increment language feature. A `track` issue is the
  track's spine, a **draft PR** settles the doc, and slice proposals are opened
  as sub-issues of the spine. Unlike a proposal, a track doc is not consumed on
  merge; it is the living map the per-slice proposals are cut from.
- [`decisions/`](decisions/README.md) — the **decision records**: one ADR per
  language-defining call, harvested from the retired increment instalments and
  added per increment going forward.
- [`reviews/`](reviews/README.md) — dated, point-in-time architecture and
  code-quality reviews. Snapshots, not standing documents; a citing track or
  trajectory re-measures rather than trusting a review to still be current.
- [`archive/`](archive/README.md) — superseded and shelved docs, kept for the
  record. Nothing there is current. Includes the retired PhD exploratory memo,
  dropped by the [positioning decision](bynk-positioning.md).

## Versioning & release

> **Cadence:** daily increments each cut a version; they batch into releases,
> tagged at each **track retirement** before 1.0 and as named **monthly
> milestones** from 1.0, with cumulative migration notes (and, post-1.0,
> codemods) — the upgrade path an outside user follows. See
> [`bynk-release-discipline.md`](bynk-release-discipline.md) (#540 §7(4)).

The repo carries a **single version** while everything lives together. The
sites that must agree — the Cargo workspace (`[workspace.package]` plus the
in-workspace dependency requirements), `vscode-bynk`'s `version`, and
`tree-sitter-bynk` — are all set by one command:

```sh
scripts/bump-version.sh X.Y.Z
```

One versioned value is deliberately **not** on that list: the extension's
`bynkServerVersion`, the GitHub Release it downloads `bynkc-lsp` from. It names
the **last shipped release**, not the workspace version (#1673). Tying it to the
workspace version meant that, between releases, it named a release that didn't
exist, and a freshly packaged VSIX couldn't download its server. `release.yml`'s
`server-pin` job moves it to the new tag once that GitHub Release is published.
`scripts/check-server-pin.sh` checks that the pin's release exists, isn't a
draft, and carries `SHA256SUMS` and a server binary for every target
`release.yml` builds. It runs in ci.yml's `server-pin` job, which has no path
filter, so it runs on every CI run and on the weekly schedule (a release can
stop being valid without a change in the repo), and again, strictly, in the
release's `verify` job. `xtask/tests/server_pin.rs` checks offline that the
pin is never ahead of the workspace, that the bump script leaves it alone, and
that `scripts/next-server-pin.sh` only moves it forward to a release tag.

The extension judges a server it downloaded against the pin, and one from
`PATH` or `bynk.executablePath` against its own version, so a contributor
running a `bynkc-lsp` built from the same tree isn't warned that it is newer
than the last release.

Per release:

1. The **implementing PR** runs the bump script and lands the version bump
   with the increment (alongside the spec/changelog deltas).
2. To ship a version, **tag `vX.Y.Z`** — the release workflow then does the
   whole release from that one tag: `verify` (tag/version match + the commit
   being on `main`) → the test suite on all three host OSes →
   build the binaries + cut the GitHub Release, **and** publish the crates to
   crates.io and the grammar to npm (both via OIDC Trusted Publishing, both
   re-run-safe — a version already on a registry is skipped, so a partial
   publish can be retried by re-running the run).
3. A release tag is cut when a track retires (before 1.0) or at each monthly
   milestone (from 1.0), not every increment; see
   [`bynk-release-discipline.md`](bynk-release-discipline.md). Once the release is published, the workflow moves the extension's
   `bynkServerVersion` to it, so the pin on `main` always names a release that
   exists. A manual `workflow_dispatch` against the tag re-runs just the
   registry publishes (the override / retry path).

The release workflow's `verify` job refuses a tag whose version does not
match **all** of the sites above. The registry publishes are irreversible; the
`verify` + `test` gates stand in for an approval gate (an Environment with
required reviewers can add a one-click pause once the repo is public).

## Branches

Cut every branch from freshly-pulled `main`, and never reuse one after it has
merged. Merges are squash-only with `delete_branch_on_merge`, so a reused branch
restarts from a base `main` no longer contains — and the PR history stops
meaning anything: `claude/bynk-fmt-line-length-akrp7n` was the head of four
separate PRs across two days, one of which (#971) was a Windows CI fix that the
branch name actively misdescribes.

Name branches `<kind>/<slug>`, where `<kind>` is one of `feat`, `fix`, `track`,
`chore`, or `docs`, and the slug describes *this* change.

## History

The per-increment grammar instalments (`grammar-increments/`,
`bynk-adapters-spec.md`) have been **removed**: the normative spec in
`site/src/content/docs/book/spec/` is the single source of truth for the shipped language, updated
in place per increment. The instalments' history lives in version control; the
design decisions they recorded live on in [`decisions/`](decisions/README.md).
