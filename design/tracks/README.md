# Feature tracks

Persistent design docs for **far-reaching, multi-increment language features** —
the artefact introduced by [ADR 0076](../decisions/0076-feature-track-posture.md),
run GitHub-native per [ADR 0167](../decisions/0167-feature-tracks-run-github-native.md):
the track's *state* lives on the tracker (a spine issue, its sub-issues, PR draft
status); the committed doc here carries the *design*.

A feature track applies when a feature has two or more of: it spans several
increments, its surface is not yet settled, or it is a security/safety boundary.
For everything else, the standard single-increment
[proposal](../proposals/README.md) still applies.

## What a track doc is — and isn't

- **Persistent, not transient.** Unlike a proposal (an issue closed by the PR
  that implements it), a track doc lives for the whole feature: it is the living
  map the per-slice proposals are cut from, updated as each slice lands, retired
  only when the theme completes.
- **A realisation of the design notes, not a replacement.** It sharpens the
  conceptual commitment in `../bynk-design-notes.md` into a concrete surface,
  an internal architecture, a security/threat model, and an ordered slice
  decomposition. The design notes stay the north star.
- **Not a build authorisation.** Merging a track doc settles *direction*. Each
  slice is still an ordinary [increment proposal](../proposals/README.md) —
  an issue citing this doc and the foundational ADRs; *accepting that proposal*
  (label `accepted`) is the approval to build.

A track whose theme is one phase of a longer programme names its **trajectory
document** ([`../bynk-compiler-trajectory.md`](../bynk-compiler-trajectory.md)
is the first) as its theme, rather than a design-notes section. The trajectory
holds the endpoint and the phase ordering; the track holds the *method* for its
own phases. Both are living documents, but only the track retires when its
slices land.

## Lifecycle

1. **Propose.** Open a **track issue** from the
   [feature-track template](../../.github/ISSUE_TEMPLATE/feature-track.md)
   (label `track`): the theme, the ADR 0076 trigger check, the open design
   questions, and the candidate slice decomposition. This issue is the track's
   **spine** — it stays open for the track's whole life, collects the slice
   proposals as **sub-issues** (GitHub's progress bar is the live slice
   status), and is closed only at retirement.
2. **Settle.** Open a **draft PR** adding `tracks/<slug>.md`, referencing the
   spine (*"Part of #\<n\>"* — never `Closes`, which would kill the spine at
   adoption). The PR's draft status *is* the settling phase: close the open
   design questions (investigation + prior art) under line-anchored review,
   and identify the load-bearing, hard-to-reverse **ADRs to land up front**.
   Marking the PR **ready for review** asserts the questions are closed;
   **merging it settles direction** — it is not a build authorisation. (If a
   settling PR is marked ready and merged without that assertion actually
   being tested under review, the track's real phase stays **Settling** past
   the merge until a follow-up re-settling PR — step 4 — closes the doc's
   open questions for real.)
3. **Slice.** Cut each increment as an ordinary
   [increment proposal](../proposals/README.md), opened as a **sub-issue of
   the spine** and citing the doc and the foundational ADRs; `accepted` on
   that sub-issue is the approval to build, and the implementing PR closes it.
   Marking the slice done in the doc (and on the spine's checklist) rides the
   implementing PR — a routine update needing no ceremony of its own.
4. **Re-settle when direction changes.** A *substantive* change of direction
   after adoption — a surface decision reversed, a slice re-scoped away, a new
   phase — gets its own small reviewed PR against the doc (a mini settling
   pass), not a ride-along on an implementing PR.
5. **Retire.** When the last slice lands, a **retirement PR** removes the doc,
   appends its closing summary to
   [`../archive/retired-tracks.md`](../archive/retired-tracks.md), and closes
   the spine (`Closes #<n>`). The decisions live on in the ADRs and the
   spec-in-place.
6. **Release.** Before 1.0, a track's retirement is the release trigger
   ([`../bynk-release-discipline.md`](../bynk-release-discipline.md), #1673):
   once the retirement PR merges and `main` is green, the maintainer tags the
   version on `main` (`vX.Y.Z`), and `release.yml` ships it.

## Active tracks

Live state — which slices are done, what is settling, what is next — is on
each track's spine issue; this table is deliberately just the map.

| Track doc | Spine issue | Phase | Theme |
|---|---|---|---|
| [`documentation.md`](documentation.md) | [#557](https://github.com/accuser/bynk/issues/557) | Slicing (slice 0 shipped) | Documentation & web presence: the Astro + Starlight migration, a CI snippet-verification harness, playground integration seams |
| [`end-to-end-testing.md`](end-to-end-testing.md) | [#2133](https://github.com/bynk-lang/bynk/issues/2133) | Settled (#2140); slicing next | End-to-end testing in the language: a project's whole-system scenarios as `system` cases — suite configuration, a `Signature` signer, `cron`/`queue` and a harness clock at `system`, then the same cases run against `bynk dev` or a deployed preview |
| [`idempotency-capability.md`](idempotency-capability.md) | [#921](https://github.com/accuser/bynk/issues/921) | Slicing (slice 0 shipped, #929; call-site key scoping follow-up shipped, #934) | The `Idempotency` capability: mechanical dedup for at-least-once delivery, per design notes §4, §12 |
| [`post-trajectory-crate-hygiene.md`](post-trajectory-crate-hygiene.md) | [#1533](https://github.com/accuser/bynk/issues/1533) | Settling | READMEs, crate-local unit tests, comment rewrite, and module decomposition left open after the compiler-trajectory's eight phases closed — internal-architecture theme, not language surface, continuing the posture `compiler-architecture.md`/`crate-decomposition.md`/`content-ownership.md` took |
| [`standard-library.md`](standard-library.md) | [#1936](https://github.com/accuser/bynk/issues/1936) | Slicing (S0 shipped, #1940) | The standard library: its four layers (kernel, commons, the `bynk` surface, `bynk.cloudflare`), its placement in a `bynk-std` crate and its 1.0 promise, derived capabilities for context-owned platform resources (`capability Sessions = Kv`), and the order in which the Workers surface is admitted |

(`documentation.md` pre-dates the GitHub-native flow, so its doc was
committed by an ordinary PR rather than a settling draft PR; the spine issue
was opened retroactively. `lsp-foundations.md` (now retired) was the first
track to run the ADR 0167 flow from the start — spine issue first, doc via a
settling draft PR; `testing-the-boundary.md` (now retired) was the second;
`locale-capability.md` (now retired) was the third; `message-bundles.md`
(now retired) was the fourth; `idempotency-capability.md` is the fifth;
`compiler-architecture.md` (now retired) was the sixth, and its theme was
internal architecture rather than language surface — as `crate-decomposition.md`
and `increment-allocation.md`, both retired, were before it.
`identity-and-totality.md` (now retired) was the seventh, continuing
`compiler-architecture.md`'s internal-architecture theme as phase 3 of the
same trajectory — all nine of its slices shipped, closing R2.2, R2.4
(expressions), R3.10, R4.1, R4.2, and R4.3 in full, R2.5/R4.9 functionally but
not structurally (see `retired-tracks.md`'s closing summary for the open
question that leaves). Its retirement also closed a documentation gap
`compiler-architecture.md`'s own
retirement PR left open: that PR ([#996](https://github.com/accuser/bynk/issues/996)'s
closing commit, `85bc851e`) removed the doc and appended its closing summary
to `retired-tracks.md` but did not update this table or its "Retired tracks"
list below at the time — both were caught up alongside opening the phase-3
row, which has itself now retired in turn.
`content-ownership.md` (now retired) was the eighth, continuing the
internal-architecture theme rather than language surface, same as
`compiler-architecture.md`/`identity-and-totality.md` before it — realising
R2.3 for `bynk-emit`/`bynk-ide`, open since T0.7.
`project-model.md` (now retired) was the ninth, continuing the same
internal-architecture theme as phase 4 of the same trajectory, entry-gated on
`identity-and-totality.md`'s own retirement note. Its settling PR
([#1108](https://github.com/accuser/bynk/pull/1108)) closed all six of its
design questions under review; one (whether `bynk-ide`'s real dependency on
`bynk-emit` was a relocatable discovery function or something bigger) surfaced
a finding the draft hadn't anticipated — `analyse_project_with` calls the same
checking-and-discovery orchestration function (`run_checks`) the CLI path
does — and settled on a new `bynk-check` analysis entry point rather than a
simple relocation, with the resulting duplication named as phase 5's to
remove. All three named slices shipped (P4.0, P4.1, P4.2), closing R3.7,
R3.8, R3.9 and R10.2 in full (R3.11 separately found already closed by prior
paydown); `ide_emit_edge` reads absent.
`semantics-in-the-checker.md` (now retired) was the tenth, phase 5 of the
same trajectory, entry-gated on `project-model.md`'s own retirement note.
Its settling PR ([#1127](https://github.com/accuser/bynk/pull/1127)) closed
all five of its design questions under review; the real finding was in Q2 —
the emission/checking boundary phase 4's own P4.1 slice drew informally,
without a settling review, turned out to already be a named, fixture-pinned,
CHANGELOG-documented live regression in the editor's own diagnostics
(`bynk-check/src/analysis.rs`'s own seven-category accounting), not merely
an architectural judgment call worth re-examining. Q3 folded into that same
finding. All six named slices shipped (P5.0–P5.5), closing R3.5 in full;
`emit_diagnostics` reads 4/6 (true/naive), true=4 the named floor — see
`retired-tracks.md`'s closing summary for why naive doesn't also reach 4.
`the-ir.md` (now retired) was the eleventh, phase 6 of the same trajectory,
entry-gated on `semantics-in-the-checker.md`'s own retirement note ("Opens
phase 6 (the IR, per the trajectory)"). Spine
[#1137](https://github.com/accuser/bynk/issues/1137); its settling draft PR
argued six design questions, including a real finding of its own — the
reference's own target crate graph (`bynk-greenfield-compiler.md` Part 10)
names `bynk-ir`/`bynk-lower` crates the trajectory's own phase-6 prose never
mentions building, settled as deliberately deferred to phase 7 rather than
built now (R10.3's own "carve at the moment a dependency appears"
discipline). Its own gated probe, `ast_importers`, retired at a re-settled
floor of 5, not the 0 first named — see `retired-tracks.md`'s closing
summary for the per-file argument and the phase-7 entry-condition amendment
this forced.
`the-typescript-tree.md` (now retired) was the twelfth, phase 7 of the same
trajectory, entry-gated on `the-ir.md`'s own retirement note and its P6.58
amendment — `ast_importers` reading its re-settled floor of 5, not 0, since
the five surviving files were this track's own future surface. Spine
[#1293](https://github.com/accuser/bynk/issues/1293); its settling PR closed
all five of its design questions under review. Two of the five changed shape
during settling, in the direction of more evidence, not more doubt: Q3
(`TsType::Any` elimination) was framed as a risk of re-opening phase 6's IR,
but a site-by-site classification of every `as any`/bare-`: any` occurrence
found full elimination achievable with zero IR work and only a small, named
residual deferred to R7.7's runtime-typing work. Q4 (R8's scope) was framed
as a closed/open binary, but a rule-by-rule audit against the current tree
found a three-way split. Q1 settled on carving `bynk-ts` as a crate up front
(the reverse of `the-ir.md`'s own `ir.rs`-first precedent). Q2 settled on a
statement-level `Verbatim` escape hatch with a closed `VerbatimOrigin` enum
and a companion textual lint, since golden fixtures alone can't see inside
an opaque hatch node. Run as six arcs (A–F, dozens of slices, far beyond the
37 first estimated at settling), all four of its own gated probes retired at
an argued floor, not the flat zero first proposed, the same honesty
`ast_importers` (floor 5) already modelled: `ts_any` at 26, `verbatim_sites`
at 2, `ts_writes` at 809, `verbatim_origins` at 1 — see `retired-tracks.md`'s
closing summary for the full, file-by-file argument behind each.
`incrementality.md` (now retired) was the thirteenth, phase 8 of the same
trajectory — the last phase — entry-gated on `the-typescript-tree.md`'s own
retirement note, which named phases 3 and 4 *together with* itself as this
phase's real precondition rather than the trajectory's own §4 diagram alone.
Spine [#1507](https://github.com/accuser/bynk/issues/1507); its settling PR
closed all five of its design questions under review. Three (Q1, Q2, Q4)
each turned on a concrete fact the draft hadn't yet checked, in every case
narrowing the option space rather than reversing the draft's own leaning.
Q1 (`UnitSignature`'s shape) found no existing type in the workspace is
close enough to widen — `combined_types_for` only ever computed one of
design notes §15's four required categories, and every closer candidate
(`UnitTable`'s own `FnDecl`/`Handler` types) carries a full body — so
`UnitSignature` is a new type wrapping `combined_types_for`'s output
unchanged plus fresh projections read from `UnitTable`, leaving all 7 of
`combined_types_for`'s existing call sites untouched. Q2 (the file level)
found that sharing completion's own `PROJECT_UNIT_CACHE` with the
diagnostics path was never actually available — `bynk-check` cannot depend
on `bynk-ide`, the crate graph runs the other way — so the settled answer
builds one new, shared `Tokens(FileId)`/`Ast(FileId)` cache in
`bynk-project` instead, which also needs a durable path↔`FileId` interning
table since `FileId` today is reallocated fresh on every parse call, not
interned across them. Q3 settled that this track builds no memo table at
all — R3.15's scheduler decision defers whole, resolved by Q5 settling the
gated probe as an existence-and-proof check rather than a latency number
that would otherwise need one to measure anything. Q4 audited
`UnitSignature`'s field list against the real `FnDecl`/`Handler`/
`StoreField` shapes, field by field, excluding every body or body-adjacent
one (contracts, storage initialisers). All six settled slices shipped
(P8.0–P8.5); its own gated probe, `incremental_query_types`, retired at
**satisfied**, not a floor — the first probe on this trajectory shaped as a
proof rather than a shrinking count. Two implementing slices (P8.3, P8.4)
and one (P8.5) each surfaced a real fork the settling review hadn't
examined, each recorded in its own ADR (0415–0417) rather than silently
absorbed. Retired 30 August 2026 — **this being the trajectory's own last
phase, this retirement also closes
[`../bynk-compiler-trajectory.md`](../bynk-compiler-trajectory.md) itself**,
per that document's own §1 endpoint and this track's own §12. Full closing
summary in `../archive/retired-tracks.md`.
`agent-capability-encapsulation.md` is a committed Draft that appears in
neither this table nor `retired-tracks.md`; it predates this row's addition
and needs a spine issue or a retirement — tracked separately, not by this
table. `idempotency-capability.md`'s settling PR (#922) was marked ready for
review and merged 55 seconds later with no review (`reviews: []`) — the
step-2 carve-out above applied to it for a time, since the "ready for
review" assertion that §3's questions were closed had never actually been
tested. A genuine re-settling pass (#924, #927) closed all four of §3's
questions for real (three settled, one deferred to a future track) before
slice 0 (#929) shipped, so the carve-out no longer applies to this row.)

## Retired tracks

A retired track's closing summary — what shipped, which ADRs carry its
decisions, the named follow-ons — is kept for the record in
[`../archive/retired-tracks.md`](../archive/retired-tracks.md):

- **`interop.md`** — spine [#1973](https://github.com/bynk-lang/bynk/issues/1973): adopting Bynk
  beside existing TypeScript Workers. A live forgery defect found while settling was fixed first
  (per-deployment call secret, ADR 0479); then wire format version 1 was published (ADR 0483), every
  internal request signed per edge (ADR 0485), foreign callers admitted with a `Foreign` actor scheme
  (ADR 0486), a typed TypeScript client generated for them, and experimental `bynk.cloudflare`
  `Service` added for calling a non-Bynk Worker (ADR 0480). Six slices shipped (v0.335.0–v0.343.0).
  Retired 17 October 2026.
- **Toolchain pins as gates** (no track doc) — spine
  [#1670](https://github.com/accuser/bynk/issues/1670), opened from the
  [2026-10-01 review](../reviews/2026-10-01-language-implementation-review.md): every external-tool
  version the toolchain states to users is gated or covered by a written policy. Seven slices shipped
  (v0.303.1–v0.303.4; ADR 0434): the Node 22 floor, TypeScript 5/7 verification, the compiler-skew
  gate, one test harness for PR and release, the `wasm-bindgen-cli` pin guard, the compatibility-date
  policy, and the extension's server pin decoupled from the workspace version, with the pre-1.0 release
  cadence (a release at each track retirement). Retired 5 October 2026.
- **`runtime-semantics.md`** — spine [#1648](https://github.com/accuser/bynk/issues/1648), opened
  from the [2026-10-01 review](../reviews/2026-10-01-language-implementation-review.md): emitted
  programs do what the spec says, and the gates check it. Sixteen correctness slices (S0–S15), #291, and four
  gates shipped (v0.290.2–v0.302.0; ADRs 0421–0433). The headline changes were structural `==`, agent
  state in the wire shape, regex admissibility, the safe-integer `Int`, behavioural fixtures that run a
  fixture's suites, and diagnostic recovery. Retired 4 October 2026.
- **`the-ir-cutover.md`** — the follow-on to phase 6 of
  [`../bynk-compiler-trajectory.md`](../bynk-compiler-trajectory.md), spine
  [#1542](https://github.com/accuser/bynk/issues/1542): opened to wire `bynk-lower`'s unconsumed
  expression lowering into `bynk-emit`, and retired on the opposite decision once its own Slice 3.2
  measured the cutover as a second code generator, not a retype. Slices 1 and 3.1 shipped as planned;
  the re-settling (§10, [#1573](https://github.com/accuser/bynk/pull/1573)) chose deletion over
  parity, and D0–D3 executed it: the one production detour into the lowerer repointed, 48
  `bynk-lower` functions and 23 `bynk-ir` items deleted (10,195 → 2,123 and 1,923 → 634 lines), the
  refusal recorded in `bynk-greenfield-compiler.md` Part 15.1 with a trigger, and the gated
  `unconsumed_ir_items` adoption probe added. Retired 2 September 2026.
- **`incrementality.md`** — phase 8 of
  [`../bynk-compiler-trajectory.md`](../bynk-compiler-trajectory.md), spine
  [#1507](https://github.com/accuser/bynk/issues/1507), **the trajectory's last phase**: every
  compiler output decomposed to the granularity at which it is invalidated —
  `Tokens(FileId)`/`Ast(FileId)`, `UnitSignature(UnitId)`, `Body(DefId)`/`TypeOf(DefId)`,
  `ProjectGraph` — with `UnitSignature` proved stable under a body-only edit (the R3.14 firewall).
  Settled across five design questions; three (Q1, Q2, Q4) each turned on a concrete fact the
  draft hadn't checked, narrowing rather than reversing it. All six slices shipped (P8.0–P8.5);
  `incremental_query_types` retires at **satisfied**, the first gated probe on this trajectory
  shaped as a proof rather than a shrinking count; `keystroke_latency` stays trend-only,
  "not measured," per this phase's own settled scope (no scheduler ships here). Six ADRs,
  0412–0417 — three front-loaded at settling, three more recording real forks P8.3–P8.5 each
  found. Retired 30 August 2026. **This retirement also closes
  [`../bynk-compiler-trajectory.md`](../bynk-compiler-trajectory.md) itself** — its own §1 endpoint
  is reached, the last of the trajectory's eight phases. Full closing summary in
  `../archive/retired-tracks.md`.
- **`the-typescript-tree.md`** — phase 7 of
  [`../bynk-compiler-trajectory.md`](../bynk-compiler-trajectory.md), spine
  [#1293](https://github.com/accuser/bynk/issues/1293): a new `bynk-ts` crate (tree, printer,
  source map) becomes the single writer of every generated TypeScript character. Run as six arcs
  (A–F); all four gated probes retire at an argued floor — `ts_any` 26, `verbatim_sites` 2,
  `ts_writes` 809, `verbatim_origins` 1 — the same honesty `ast_importers` (floor 5) already
  modelled for phase 6. Twenty-one ADRs, 0385–0410 (minus five numbers belonging to a concurrent,
  unrelated track). Retired 29 August 2026. Opens phase 8, once its own settling review grounds a
  fresh spine against the current tree. Full closing summary in `../archive/retired-tracks.md`.
- **`the-ir.md`** — phase 6 of
  [`../bynk-compiler-trajectory.md`](../bynk-compiler-trajectory.md), spine
  [#1137](https://github.com/accuser/bynk/issues/1137): `bynk-emit` gains a typed IR
  (`ir.rs`/`ir/lower.rs`), the emitter's own dispatch decisions move off re-derived AST
  name-matching onto resolved `Callee`/`IrItem`/`TyId` reads. Run as three arcs (IR construction,
  completion, retirement), ~59 slices. `ast_importers` retires at a re-settled floor of 5, not 0
  — `bynk-emit/src/emitter{,/**}` exactly, argued file by file. Fifty-one ADRs, 0332–0382. Retired
  19 August 2026. Opens phase 7 (`the-typescript-tree.md`) inheriting this floor as a named,
  argued rendering-subtree boundary. Full closing summary in `../archive/retired-tracks.md`.
- **`semantics-in-the-checker.md`** — phase 5 of
  [`../bynk-compiler-trajectory.md`](../bynk-compiler-trajectory.md), opened directly by
  `project-model.md`'s own retirement note below: every remaining whole-project check still in
  `bynk-emit` relocates to `bynk-check`, so `bynk-check` is the one crate that checks (R3.5). All six
  slices shipped (P5.0–P5.5), closing R3.5 in full (R4.6, R4.11, R10.4 reverified still closed, not
  reopened; R10.1 closed by a crate-doc correction, not a split). Three ADRs:
  [0329](../decisions/0329-semantics-phase5-rule-scope.md)–[0331](../decisions/0331-semantics-phase5-function-boundary-hook.md).
  `emit_diagnostics` reads 4/6 (true/naive) — true=4 is the named floor (four `#[cfg(test)]` assertion
  strings, not live diagnostic construction). Retired 9 August 2026. Opens phase 6 (the IR, per the
  trajectory). Full closing summary in `../archive/retired-tracks.md`.
- **`project-model.md`** — phase 4 of
  [`../bynk-compiler-trajectory.md`](../bynk-compiler-trajectory.md): the project model (discovery, the
  unit graph, the schema registry) moved below both `bynk-check` and `bynk-emit` into its own crate,
  `bynk-project`; `bynk-ide` repointed at a new `bynk-check` analysis entry point instead of
  `bynk-emit`, closing R10.2. All three slices shipped (P4.0, P4.1, P4.2), closing R3.7, R3.8, R3.9 and
  R10.2 in full (R3.11 separately found already closed by prior paydown). Three ADRs:
  [0326](../decisions/0326-project-model-phase4-scope.md)–[0328](../decisions/0328-project-model-analysis-entry-point.md).
  `ide_emit_edge` reads absent. Retired 8 August 2026. Opens phase 5 (semantics centralisation —
  `validate.rs` dissolves into `bynk-check`).
- **`content-ownership.md`** — `bynk-lsp` becomes the sole reader of `.bynk`
  project source content, realising R2.3 for `bynk-emit`/`bynk-ide`. All six
  slices shipped (0–5); `fs_below_driver` reads 0 for `bynk-ide`, 3 for
  `bynk-emit` (an enumeration walk plus two deliberate, permanent carve-outs
  — adapter `.binding.ts` reads and the plain manifest reader's own
  `bynk.toml` read — filed as [#1104](https://github.com/accuser/bynk/issues/1104),
  a probe-precision follow-on, not reopening either decision). One new ADR,
  [0325](../decisions/0325-content-ownership-seam-simplification.md),
  superseding [0322](../decisions/0322-content-ownership-seam-type.md).
  Retired 6 August 2026.
- **`identity-and-totality.md`** — phase 3 of
  [`../bynk-compiler-trajectory.md`](../bynk-compiler-trajectory.md), opened directly by
  `compiler-architecture.md`'s retirement below: `ExprId`/`FileId` node identity, `Ty::Error` and
  total `expr_types`, `Ty` interned behind `Copy`-cheap `TyId`, and `certify`/`CheckedProgram` as
  the analysis/emission boundary. Settled 3 August 2026; all nine slices shipped (T3.0, T3.3a,
  T3.3b, T3.4, T3.5, T3.6a, T3.7a, T3.7b, T3.6b), closing R2.2, R2.4 (expressions), R3.10, R4.1,
  R4.2, and R4.3 — R2.5/R4.9 functionally but not structurally (`expr_types` stays a `HashMap`, not
  the `IndexVec` those rules literally name; a named open question, not silent residue, per the full
  closing summary). Decisions in ADRs
  [0316](../decisions/0316-ty-interning-interior-mutability.md)–[0318](../decisions/0318-ty-interning-one-table-per-build.md).
  Retired 4 August 2026. Opens phase 4 (`bynk-project`, per the trajectory).
- **`compiler-architecture.md`** — phases 0–2 of
  [`../bynk-compiler-trajectory.md`](../bynk-compiler-trajectory.md): the compiler testable at crate
  granularity (phase 0, Seams), the named small defects closed with the registries single-sourced
  (phase 1, Paydown), and `lower_expr` returning what it produced instead of appending it to a
  caller-supplied sink (phase 2, Typed lowering) — no language surface change. Settled 1 August 2026
  (settling PR [#997](https://github.com/accuser/bynk/pull/997), v0.246.1); ten Tier A slices plus the
  decision slice and Tier B's three typed-hoist slices all shipped, v0.246.2–v0.247.4. Retired
  3 August 2026 once Tier B's probe (`hoist_sinks`) read zero and both of ADR 0311 D3's phase-3
  triggers had fired. Decisions in ADRs
  [0309](../decisions/0309-refactor-acceptance-gate-per-tier.md),
  [0310](../decisions/0310-the-emit-abi-is-published-the-codegen-is-not.md),
  [0311](../decisions/0311-the-lowering-substrate.md), and
  [0312](../decisions/0312-narrow-bynkc-public-api.md). Opens directly into
  `identity-and-totality.md` above (phase 3, spine
  [#1046](https://github.com/accuser/bynk/issues/1046), now itself retired); three named residue
  items filed as
  [#1047](https://github.com/accuser/bynk/issues/1047)–[#1049](https://github.com/accuser/bynk/issues/1049).
  Full closing summary in `../archive/retired-tracks.md`.
- **`message-bundles.md`** — the `messages` construct, the checked catalogue,
  and the bundle lookup that turned the shipped, bundle-free `render` (ADR
  0256) into a real localiser — the sibling `locale-capability.md` named but
  left unfiled. All three slices shipped: the construct + a single-reference
  bundle, multi-locale completeness + placeholder agreement, and ICU
  MessageFormat (`plural`/`select`/`number`/`date`, host-`Intl` delegation,
  no CLDR data bundled); shipped v0.228.0–v0.230.0 (ADRs 0272–0273, 0276).
- **`locale-capability.md`** — Bynk's first i18n surface: an ambient `Locale`
  capability paired with a pure, total `render(tag, msg) -> String`. All
  three named slices resolved: the capability + `LocaleTag`/`Message`/
  `render` (slice 1), real Cloudflare `Accept-Language` negotiation (slice
  2, shipped with a `uses`-collision limitation later closed by a
  `bynk.locale.types` leaf-commons split), and ICU MessageFormat retired in
  favour of message-bundles' own slice 3; shipped v0.221.0–v0.232.0 (ADRs
  0256, 0277–0278). The track's own stated payoff — automatic boundary-codec
  integration from a refinement failure to a localised message — never
  shipped: it depends on the still-unfiled `predicate`-declaration language
  change; every `render` call across all three slices is manual,
  handler-authored.
- **`deploy.md`** — the `bynk deploy` verb: provisioning + remote deploy, and
  the load-bearing provisioning-state model (`bynk.deploy.lock`); the first
  driver command with irreversible, outward-facing side effects. All six
  slices shipped: state model + KV MVP, DO/queue provisioning, multi-context
  ordering, secrets, environments, and reconciliation maturity + `--prune`;
  shipped v0.154–v0.220.2 (ADRs 0179–0180, 0193–0196, 0254–0255).
- **`increment-allocation.md`** — the version and the ADR number, two serial
  counters that collided across parallel increments, are now assigned by a
  per-merge stamp on `main` (an `xtask` command + a workflow) from a numberless
  `design/pending/` file, not chosen in the feature PR; the surface-shrink also
  made `llms-full.txt` a build artifact. Slices 0–3 shipped (ADR 0206).
- **`lsp-foundations.md`** — the foundation under the shipped LSP surface: one
  project model shared with `bynkc`, a freshness contract, real multi-root
  workspaces, startup analysis + server-registered watchers, one scheduler, and
  the `[lib]` test seam; shipped v0.175–v0.184 (ADRs 0198, 0201, 0202, 0204).
- **`editor-currency.md`** — the editor surface (hover, completion, scaffolds,
  menus/keybindings, codelens) brought back in step with the language and held
  there by a mechanical floor; shipped v0.121–v0.129 (ADRs 0156–0157).
- **`testing.md`** — one predicate surface: examples, properties, contracts,
  invariants, and interaction checks unified over the invariant predicate;
  shipped v0.112–v0.119 (ADRs 0144–0155).
- **`in-browser.md`** — the Browser platform, the JS emit path, the wasm
  toolchain, and the in-browser REPL/playground; shipped v0.108.0–.5
  (ADRs 0136–0140).
- **`websocket.md`** — real-time Bynk: `Stream[T]`, streaming HTTP,
  held-resource linearity, and the `from WebSocket` protocol; shipped
  v0.100–v0.107 (ADRs 0128–0135).
- **`storage.md`** — the agent-local storage-kind catalogue: `store` fields,
  the five kinds, the parity cutover, rehydration validation; shipped
  v0.82–v0.97 (ADRs 0108–0113, 0121–0124).
- **`query-algebra.md`** — the read/transform combinator vocabulary:
  `Query[T]`, `@indexed`, joins & grouping; shipped v0.88–v0.94
  (ADRs 0114–0120).
- **`debugging.md`** — source-mapped step debugging, phase 1 + the phase-2
  on-ramp; shipped v0.67–v0.73 (ADRs 0103–0104).
- **`semantic-debugging.md`** — the debugger speaks Bynk: variables, scopes,
  and the call stack rewritten to Bynk's vocabulary; shipped v0.74–v0.77
  (ADR 0105).
- **`crate-decomposition.md`** — `bynkc` decomposed into the layered library
  set; shipped v0.60–v0.66 (ADRs 0099–0102 + the 0084 amendment).
- **`actors.md`** — actor declarations as boundary contracts; the inaugural
  feature track, Q1–Q7 shipped v0.45–v0.54 (ADRs 0080–0082, 0085, 0088–0092).
- **`lsp.md`** — the editor-experience connective plan; slices 0–7 + 9 shipped
  from v0.24 (ADRs 0093–0095), the feature spec living on in
  [`../bynk-lsp-spec.md`](../bynk-lsp-spec.md).
