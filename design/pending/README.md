# Pending increments

The mechanism from the **increment-allocation** track (now retired — closing
summary in [`../archive/retired-tracks.md`](../archive/retired-tracks.md), spine
[#685](https://github.com/accuser/bynk/issues/685)); the load-bearing decision is
[ADR 0206](../decisions/0206-allocation-on-main.md).

A feature PR does **not** choose its version or its ADR number. Both are
strictly-increasing counters, and a value picked while a PR is open races every
other open PR — the merge order that alone can assign them correctly is not yet
known, so two parallel increments conflict and the loser silently ships a
number another increment already took.

Instead, a feature PR declares its **intent** in one file here, and a merge-time
stamp (a later slice) assigns the numbers in merge order. Because each pending
file is uniquely named per branch and touches none of the shared version/ADR
files, two parallel PRs have no shared line to conflict on.

## The file

Add one `design/pending/<slug>.md`, where `<slug>` is a kebab-case name unique to
your branch (e.g. the branch name):

```markdown
---
level: minor
changelog: A test case drives an http handler at the unit tier by address
---

## ADR: unit-tier-service-address
title: A test case drives an http handler at the unit tier by address
summary: How a case addresses a handler and names its principal

**Context.** …

**Decision.** …

**Consequences.** …
```

### Frontmatter (required)

- **`level`** — `minor` (a language increment) or `patch` (non-language). The
  stamp turns this into the next `X.Y.Z`.
- **`changelog`** — the changelog row, on one line, **without** a version number
  (the stamp prepends it). It lands verbatim in the Book's
  `reference/changelog.md` table, so it is **Markdown**, not YAML: do not wrap
  it in quotes (they would appear in the changelog, and the stamp rejects them).
  Wrap code in backticks,
  or a generic call like `Events.emit[E](event)` parses as a link to a relative
  `event` and fails the docs site's link-checker. The stamp rejects a relative
  link here, pre-merge, because the site only ever sees the row *after* it is
  written on `main`.

  That check is about link *shape*, not link *targets*. Absolute (`https://…`),
  `mailto:`, site-root (`/book/…`) and anchor (`#…`) destinations pass it
  unexamined — but the site's link-checker does resolve the last two, so a
  mistyped `/book/no-such-page/` or `#no-such-anchor` still passes the stamp and
  then fails the docs build on `main`. Check those by hand.

### Frontmatter (optional)

- **`closes_rule`** — comma-separated greenfield reference rule ids (`R2.3`)
  this increment closes; the stamp writes them to
  `design/greenfield-status-rules.md`.
- **`probes`** — comma-separated **gated** probes from
  `design/greenfield-status.md` whose reading this increment changes, e.g.
  `probes: ts_writes, diagnostic_coverage`. Name them here instead of
  regenerating the table: `greenfield_status_table_is_current` accepts a
  disagreement on an acknowledged probe (and prints the move, so review still
  sees it), and the stamp rewrites the table's gated rows on `main` in merge
  order. That keeps the shared table out of feature PRs, which is what made it
  the most-conflicted file in the repo. A PR with no increment can still run
  `cargo xtask greenfield-status --apply` and commit the table as before.
  Trend probes aren't gated, so they're never named here.

- **`breaking`** — a one-line Markdown note for an increment that breaks
  existing programs: what breaks, the diagnostic a user will see, and the fix
  (`bynk check --fix` where it is machine-applicable). Omit it when nothing
  breaks. The stamp writes it under the increment's version into the Book's
  `reference/upgrading.md` (the canonical record, newest first), and the release
  workflow assembles each release's **Upgrading from vX** section from that page
  with `cargo xtask release-notes <tag>`. Like `changelog`, it must not start
  with a version number and must not contain a relative Markdown link. Pending
  files without it stamp exactly as before.

No other keys are permitted — a typo like `levl:` is rejected rather than
silently ignored.

### ADR blocks (zero or more)

Each `## ADR: <slug>` heading opens a block, followed by a short header of key
lines and then the ADR body, which runs until the next `## ADR:` heading or end
of file. An increment that records no decision has no blocks; one that records
several has several. The stamp assigns the number `NNNN` at merge and writes
`design/decisions/NNNN-<slug>.md` — a `# NNNN — <title>` heading and a status
line, then the body verbatim — plus the `decisions/README.md` index row. So the
prose is reviewed **here**, in the feature PR, exactly as an ADR is today.

- `<slug>` is kebab-case and unique within the file; it becomes the ADR filename.
- **`title`** (required) — the one-line title for the ADR heading and the index row.
- **`summary`** (optional) — the one-line distillation for the index row;
  defaults to the title.
- **`status`** (optional) — defaults to `Accepted`.
- A block's body must not be empty.

## Validation

`cargo xtask check-pending` validates every file in this directory (this
`README.md` is skipped). It runs in CI so a malformed file fails the PR that adds
it — via the main test job on a code-touching PR, and via the `drift` job on a
`design/pending/`-only PR. The validator lives in the unpublished `xtask` crate
(`xtask/src/lib.rs`).

## The stamp

`cargo xtask stamp` reads the pending files, assigns each its next version (in
filename order) and the next ADR number(s), and — with `--apply` — runs
`scripts/bump-version.sh`, prepends the changelog row(s), writes the ADR file(s)
and index row(s), and **deletes** the consumed pending file(s). Without
`--apply` it prints the plan and changes nothing. Deleting what it consumes is
what makes a re-run a no-op. An increment with `breaking:` also gets an entry on
the Book's upgrading page.

## Lifecycle

On merge to `main`, the **stamp workflow** (`.github/workflows/stamp.yml`) runs
`cargo xtask stamp --apply` automatically: it assigns the version and ADR
number(s) in merge order, materialises them, and deletes this file — so a merged
pending file becomes a stamp commit on `main` with the bump, the changelog row,
and the numbered ADR(s). `cargo xtask stamp --apply` also remains a manual/local
run for recovery. See [ADR 0206](../decisions/0206-allocation-on-main.md).
