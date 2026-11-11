# Bynk — Release Discipline: Milestone Cadence, Doc-Truth Guards & the Compatibility Date

*Decision record for [#540](https://github.com/accuser/bynk/issues/540) §7(4), the
two bullets left after the [1.0 definition](bynk-1.0-definition.md): **monthly
milestone cadence** (from 1.0; Part 1 records the pre-1.0 trigger, #1673) and
**README/about drift guards**. Fourth of the strategy
records. A strategy record, not a language-defining call. With this, §7(4) is
closed. Part 3, the Workers compatibility-date policy, was added later for
[#1677](https://github.com/accuser/bynk/issues/1677), because it rides on the
same releases.*

---

## Why these two go together

The [1.0 definition](bynk-1.0-definition.md) makes a **stability promise**. A
promise is only worth what the discipline behind it can *keep* and *prove*:

- **Keep it** — batch the daily breaking increments into a form an outside
  upgrader can actually follow, with the migration written down (the *cadence*).
- **Prove it** — never let the docs claim something the compiler does not do,
  because *for a spec-first project, doc truth is the brand* (the *drift guards*).

Both are the release-discipline layer under the 1.0 definition: the machinery that
turns "Foundations is stable" from an intention into something a user can rely on
and verify.

## Part 1 — Monthly milestone cadence

### The bind

Bynk ships **daily breaking increments**, each cutting its own version
(`v0.142`, `v0.143`, …) with themed changelog sections and a per-increment ADR +
spec delta. That granularity is right for *building* the language and fine while
the only consumer is this repo's own corpus. But it is unusable as an **upgrade
path** for anyone outside: no one can be asked to read thirty daily changelog
entries and thread thirty separate migrations to move up a month.

### The decision

**Batch the daily increments into named monthly milestones.** A milestone is a
**rollup** over that month's increment-versions — the per-increment version, ADR,
and spec discipline underneath is unchanged; the milestone sits on top of it and
carries the two things an upgrader needs:

1. **A cumulative migration note** — the *net* delta to move from the previous
   milestone to this one, written as one coherent upgrade, not the day-by-day
   play-by-play. If a surface was added and then refined across three increments,
   the milestone note describes the surface as it lands, once.
2. **Codemods, post-1.0** — the automation for the mechanical part of a breaking
   change. [ADR 0123](decisions/0123-state-block-cutover-and-codemod.md) is the
   per-cutover template: it settled that *pre-1.0*, with the corpus the only Bynk
   source in existence, a shipped codemod would "parse a retired surface forever
   for no caller," so migration is a one-time in-repo hand rewrite. *Post-1.0*,
   when external code exists, that calculus flips: a Foundations-affecting change
   (rare, by the 1.0 definition) is expected to ship a codemod, and the milestone
   bundles it.

### Why monthly, and what it changes

Monthly is the grain that keeps the migration note **accurate** (a month's changes
are still small enough to describe honestly) while being **coarse enough** that an
outside upgrader reads *milestones*, not increments. It changes nothing about how
increments are built, versioned, or reviewed — the ADR-per-call and
spec-updated-in-place discipline stands. It adds one artefact: the monthly
milestone rollup with its cumulative migration note (and, post-1.0, codemods).

This is exactly the cadence the [1.0 definition](bynk-1.0-definition.md) leans on
— "named milestones with cumulative migration notes" — and it is what keeps the
1.0 stability promise *legible*: a 1.x user upgrades milestone to milestone, reads
one migration note, and (for anything mechanical) runs one codemod.

### Before 1.0: a release at each track retirement (#1673)

The monthly cadence was written down, but nothing said when a milestone was
due, and in practice none was cut after `v0.290.0` (2026-09-04), while `main`
moved on by more than a dozen versions.
[#1673](https://github.com/accuser/bynk/issues/1673) asked for the cadence to
be reaffirmed or replaced in writing.

**Monthly milestones are the cadence from 1.0.** Before 1.0 a calendar trigger
doesn't fit. The month is the right grain for an *outside* upgrader, and before
1.0 the only Bynk source is this repo's own corpus (the reasoning ADR 0123
applied to codemods). Work also lands in tracks, not months, and a release cut
partway through a track ships half of a change. **So before 1.0, a release is
cut when the work it ships is whole:**

1. **When a track retires.** The retirement PR is the point at which a track's
   slices are all merged and its spine closes, so the version on `main` after it
   merges is a coherent unit. The maintainer tags it (`git tag vX.Y.Z` on a
   commit whose CI is green).
2. **When a user-facing fix needs to reach users** before the track it sits in
   retires: a broken download, a crash in a released `bynkc`, a security fix.
   That release is tagged from `main` as soon as the fix merges.
3. **What the tag does.** `release.yml` does the rest from the tag: it builds
   the binaries, cuts the GitHub Release, publishes the crates and the grammar,
   moves the VS Code extension's server pin to the new release, and tells the
   bynk-lang org canary, which re-runs the org's example and action
   repositories against it.
4. **What a release carries.** The cumulative migration note (above), covering
   the increments since the previous release, and the Workers
   compatibility-date review (Part 3).

At 1.0 this section gives way to the monthly cadence above, unchanged.

**What still depends on the cadence.** The extension's server pin used to: it
was rewritten to the workspace version on every increment, so between releases
it named a release that didn't exist. It now names the last shipped release, so
a long gap between releases leaves the extension *behind* rather than broken.
What a gap still costs is currency: the GitHub Release binaries, the crates on
crates.io, and the server the extension downloads all stay at the last release,
while the README's `cargo install --path` builds whatever is on `main`.

## Part 2 — Doc-truth drift guards

### The principle

For a spec-first project the documentation **is** the contract: the normative spec
defines the language, and the README and about pages are the first promise a
prospective user reads. A doc that lies — a front-page example that no longer
compiles, a feature blurb for a surface that was retired — is not a cosmetic bug,
it is a **broken promise on the most visible surface there is**. So doc truth is
guarded by CI like any other invariant.

### What is already guarded

The drift-guard pattern is well established, and the specific README/about failures
the review named are **already closed in code**:

- **`doc_examples`** compiles every `` ```bynk `` block in the Book, the Developer
  Documentation, the landing page, **and the root `README.md`** — the last added
  after the front-page showcase "had drifted through three syntax revisions before
  this gate covered it" (`bynk-tests/tests/doc_examples.rs`). The about pages live under
  `book/` and are covered by the same gate.
- **`doc_diagnostics`** checks that quoted diagnostic output matches the compiler;
  **`doc_version`** that version references do not drift; **`grammar_reference`**
  that the EBNF matches the tree-sitter grammar; **`decisions_index`** that the ADR
  index is complete by construction; **`legend_drift`** that the LSP legend and the
  VS Code extension agree; plus the sidebar drift guard in CI.

So the review's concrete complaints — a front-page example that did not compile,
and a README advertising a retired testing surface — have both been addressed. The
open work is the **policy**, not a missing example gate.

### The standing rule (the decision)

1. **Every high-visibility surface's compilable claims are CI-gated.** README,
   landing page, and about pages are in the `doc_examples` gate and stay in; any
   new front-facing surface that ships a `` ```bynk `` example joins the gate as it
   appears. A code fence on a front-door page is a promise, and promises are
   compiled.
2. **The per-increment doc-delta discipline extends to README + about prose.** An
   increment that retires, renames, or changes a user-facing surface updates the
   README and about pages **in the same PR**, the way it already updates the spec
   and the tooling — the drift-guard proposal-template line grows a
   "front-door prose" entry. This is what would have caught the "retired testing
   surface" blurb at the point it went stale.
3. **Prose that cannot be compiled is held by discipline, and converted to
   checkable references where feasible.** A natural-language "does the README lie"
   checker is not worth building. Instead: any prose claim that maps to a
   *checkable fact* — a diagnostic code, a CLI flag, an ADR status, a spec section
   — is preferred as a **guarded reference** (a link or an included snippet the
   existing guards already police) over free-floating prose. Un-mechanizable
   claims fall to the doc-delta review discipline of rule 2.

The line this draws: we **compile every example**, make the **doc-delta a required
part of every increment** for the front-door surfaces, and **prefer checkable
references over unguarded claims** — that is the extension of the drift-guard
pattern the review asked for, without pretending to mechanize prose.

## Part 3 — The Workers compatibility date

### The bind

Every generated `wrangler.toml` pins `compatibility_date` to one compile-time
constant (`COMPATIBILITY_DATE` in `bynk-emit/src/emitter/wrangler.rs`).
Cloudflare uses the date to fix the Workers runtime behaviour a Worker sees, so
moving it can change how deployed Bynk code behaves without any change to the
compiler's output. Until #1677 the only guidance was "bump cautiously", with no
trigger for a bump. The date stayed at `2024-11-01` for 23 months, falling
further behind the runtime that Cloudflare and the local `workerd` actually
test. The first review under this policy, at the `v0.303.4` release, moved it
to `2026-07-01`; the review at the Hyperdrive H0 work ([#3317](https://github.com/bynk-lang/bynk/issues/3317)) moved it to `2026-08-04`.

### The decision

**Review the date at each release (Part 1: a track retirement before 1.0, a
monthly milestone from 1.0), and move it only once the workerd smokes pass on
the new date.** The recommended option of #1677's Decision A.

1. **Trigger.** Each release reviews the date. A review may keep the date when
   nothing is worth taking, and the release says so. The candidate is a date no
   later than any of:
   - the release itself;
   - the newest date the `workerd` behind the smokes' wrangler
     (`bynk-tests/tests/wrangler/mod.rs`'s `SPEC`) supports;
   - **the newest date supported by a wrangler released at least three months
     before the review** (the lag rule, added at the first review, #1731).

   The lag rule exists because **an older `workerd` refuses a newer date
   outright.** It doesn't fall back: `wrangler dev` exits with "This Worker
   requires compatibility date …, but the newest date supported by this server
   binary is …". A developer's wrangler is often older than the newest release,
   whether it's installed on `PATH`, pinned in the project, or extracted in the
   `npx` cache, which is keyed on the spec `wrangler@4` rather than the version
   it resolves to. A date only the newest wrangler supports would break
   `bynk dev` for all of them. Three months back keeps any wrangler from the
   last quarter working. Look the date up with `npm view wrangler time` and
   `npm view wrangler@<v> dependencies.workerd`: the `workerd` version
   `1.YYYYMMDD.n` names the newest date it supports.

   **The review also sets the wrangler minimum** (#1732): `WRANGLER_MIN`, beside
   `COMPATIBILITY_DATE`, is the oldest wrangler whose `workerd` serves the date.
   It's currently `4.120.1` for `2026-08-04`. `bynk doctor` warns about an
   installed wrangler below it, and `bynk dev` prints the same warning before
   serving. `bynk-emit/tests/compat_date_policy.rs` fails until this section,
   the doctor guide and the emission reference all name the current pair.
2. **Gate.** The bump lands as its own PR. Before it merges, the full suite runs
   with `BYNK_REQUIRE_WORKERD=1` on the new date, so the workerd smokes exercise
   the generated Workers under the new runtime behaviour. The PR lists the
   compatibility flags Cloudflare switched on between the two dates and says
   which of them, if any, the runtime or the emitted code depends on.

   **The default flag set is part of the review** (#1890).
   `DEFAULT_COMPATIBILITY_FLAGS`, beside `COMPATIBILITY_DATE`, lists the flags
   every emitted `wrangler.toml` carries ahead of a project's own
   (`bynk.toml`'s `[workers] compatibility_flags`). It's currently
   `global_fetch_strictly_public`, which makes a Bynk `Fetch` to a URL go over
   the public internet. The review checks each default against the new date:
   a flag the date now turns on is redundant and can be dropped, and a flag a
   new default would contradict needs an explicit decision.
3. **Record.** A bump changes what every deployed Worker runs under, so it
   carries a changelog entry. It needs an ADR only when a flag changes
   behaviour a Bynk program can observe.

### Why at releases

A release is the point at which an upgrader already reads one cumulative note, so a runtime change belongs there, and not in a daily increment that
nobody outside reads. The smokes are the gate because they are the only tests
that run the generated Workers on the real runtime. A golden can't see a
runtime behaviour change, since the emitted `wrangler.toml` differs only in the
date.

## Interlocks

- **With the 1.0 definition (§7(4)).** The cadence keeps the stability promise
  *legible* (milestone-to-milestone upgrades with one migration note); the drift
  guards keep the doc-truth-is-brand promise *honest*. Together they are the
  discipline that makes 1.0 credible.
- **With the sequencing decision (§7(2)).** The tooling freeze there and the
  doc-delta discipline here are the same instinct — spend effort on truth and
  adoption, not on surface the project cannot yet back.
- **With the §7(5) validation bar.** The cumulative migration note per milestone is
  exactly what the two proposed external deployments would be "carried through" to
  prove the promise holds. §7(5) is tracked on [#3055](https://github.com/bynk-lang/bynk/issues/3055).

## Out of scope here

This closes §7(4) (the 1.0 definition, the milestone cadence, and the drift
guards). The remaining §7 calls — the honest **comparison page** (§7(3)) and the
**two-production-deployment validation bar** (§7(5)). The comparison page has since
been written; §7(5) is tracked on [#3055](https://github.com/bynk-lang/bynk/issues/3055).
