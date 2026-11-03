# Crate hygiene — dependency edges, crate boundaries, the system model, and the installables

- **Status:** Settled (#2776). Every question in §11 is closed; the maintainer confirmed the
  adopted answers on 2026-11-02. Slicing next: A1–A3, C1–C2, C4a, B1 and E4a can start at once, and no slice
  is authorised until its proposal is accepted.
- **Spine:** [#2775](https://github.com/bynk-lang/bynk/issues/2775).
- **Supersedes:** `post-trajectory-crate-hygiene.md` (spine
  [#1533](https://github.com/bynk-lang/bynk/issues/1533)), withdrawn before it settled. §1.1 says
  what happens to each of its workstreams.
- **Realises:** `design/bynk-greenfield-compiler.md` R10.1 ("a crate is named for what it
  produces"), R6.13 ("Declarations are IR nodes", extended past the emitter in §6), and the
  pre-1.0 goal of shipping `bynk` and, optionally, `bynk-lsp` as the only installables.
- **Amends (via the front-loaded ADRs, §13):**
  - [ADR 0099](../decisions/0099-crate-layering-dependency-direction.md), where it names
    `bynk-grammar` the model leaf
  - [ADR 0100](../decisions/0100-structured-data-rendering-separation.md), on `bynk-render` as its own crate
  - [ADR 0101](../decisions/0101-front-end-links-pipeline-binary-topology.md), which keeps a thin
    `bynkc` binary
  - [ADR 0434](../decisions/0434-act-on-compiler-skew.md), the skew gate on a second compiler
  - Part 15.1's IR entry: its *Status* line names this track as where the adopted half grows
- **Posture:** Feature track per [ADR 0076](../decisions/0076-feature-track-posture.md). See §2.

## 1. The theme

The crate-decomposition track split the `bynkc` monolith into a layered library set (ADR 0099),
and the compiler trajectory moved code between those crates for eight phases. Nobody has since
looked at the graph as a whole. Measured at `4107c9185` (v0.480.18), the workspace has 22 crates,
18 of them published, in eight dependency layers. The longest serial build chain is
`bynk → bynk-driver → bynk-emit → bynk-lower → bynk-ir → bynk-check → bynk-project → bynk-syntax`,
about 229k of the workspace's ~304k source lines. `bynk-check` (81k) and `bynk-emit` (61k) make up
142k of it.

The graph has four kinds of residue:

1. **Dependency edges that carry almost nothing.** Measured as non-comment `bynk_x::` paths in
   the dependent crate's `src/`:
   - `bynk-lsp` depends on `bynk-emit` (and through it on `bynk-lower` and `bynk-ir`) for one
     function, `emitter::pred_condition_and_message`.
   - `bynk-lsp` and `bynk-driver` each depend on `bynk-ts` for one printer call
     (`print_expr`, `print`).
   - `bynk-check` depends on `bynk-fmt` for three calls to `expr_to_string`.
   - `bynk` declares `bynk-render` and never uses it.
2. **Crate boundaries that don't match their consumers.**
   - `bynk_emit::project` (`compile_project`, `check_project`, `analyse_in_memory_project`, the
     asset and suite discovery) is the orchestration API. It gets 125 references from outside
     `bynk-emit`, more than every other `bynk-emit` item combined. Its consumers (the CLI, the
     driver, `bynk-wasm`, `bynk-strip`) reach code generation only through it.
   - `bynk-ir` (657 lines) and `bynk-lower` (2,582) have one producer and one consumer between
     them (§6).
   - `bynk-render` (949 lines) has one production user once `bynkc` is gone: `bynk-driver`.
     Its only other user is a `bynk-fmt` dev-dependency (§8 D2).
3. **A published set wider than what's installed.**
   - `bynk-grammar` is published, and first in the release order, though only `bynkc`'s
     integration tests use it.
   - It also turns on `serde_json`'s `preserve_order` feature, which Cargo applies to every
     crate in a workspace build. `xtask/src/editor_contract.rs` already sorts keys by hand
     because of it.
   - The 40.6k-line integration corpus lives in `bynkc/tests`, so a binary crate of 640 source
     lines dev-depends on nearly every crate.
4. **Three installables where one would do.** `bynkc`, `bynk` and `bynkc-lsp` ship as release
   binaries, and `bynk` can't stand alone: `bynk test` and `bynk compile` start the `bynkc` that
   the driver resolves (`bynk/src/test.rs:131`, `bynk/src/workers.rs:53`,
   `bynk/src/shell.rs:54`). That second compiler is the only reason ADR 0434's skew gate exists.

**End state:**
- every dependency edge carries real use
- every crate has a reason to exist: a separate consumer, a separate build target, or a heavy
  dependency it keeps away from the rest
- only what an installed binary needs is published
- the release ships `bynk` and, optionally, `bynk-lsp`
- CI reports a failure from the cheapest test tier that catches it, and a costlier tier doesn't
  hold a failed PR up (§7 C4)

### 1.1 What #1533's workstreams become

| #1533 workstream | Disposition | Why |
|---|---|---|
| 1 — crate READMEs | **Done** | Every workspace crate has a `README.md` today. The layering diagrams are re-checked by §12's slices as they change the graph. |
| 2 — crate-local unit tests | **Partly carried** | Its `bynk-ir` → `bynk-check` finding (its S7) moves into §6 Q1. §7 C4b moves the corpus tests that exercise one crate's API into that crate, so they run in the unit tier. Density targets are not carried (§14). |
| 3 — comment rewrite | **Not carried** | It is independent of crate shape. Re-propose it on its own if still wanted. |
| 4 — module decomposition | **Not carried as a workstream** | Its numbers are stale: files over 2,000 lines went from 20 to 42, and `bynk-lsp/src/lib.rs` from 6,535 to 10,926. Two files move with §8's slices and split as they move (`tests_emit.rs`, 11,022 lines, and `project.rs`, 6,604). The rest needs a fresh survey. |

#1533 closes as superseded by this track's spine. Its doc is removed and its summary appended to
`design/archive/retired-tracks.md`, marked withdrawn.

## 2. Why a track (the ADR 0076 trigger)

- **Multi-increment.** Five workstreams across 15 crates and both release workflows. Several
  slices move a gated probe (§3) and must each re-stamp it alone.
- **Surface not yet settled** (at proposal; §11 records how settling closed it).
  - Retiring `bynkc` changes the distribution: `cargo install bynkc`, CI scripts that call it,
    and `vscode-bynk`'s binary lookup (ADR 0101's three contracts).
  - Where the system model sits (§6 Q1) and whether it becomes a public interface (§6 Q4) are
    open.
- Not a security/safety boundary. No language surface, runtime behaviour or capability boundary
  changes. §9's retirement of the skew gate removes a check, but only because the second
  compiler it guards goes away.

## 3. The constraint every slice is checked against: the gated probes

`xtask/src/greenfield_status.rs` computes the probes, `design/greenfield-status.md` is the committed
table, and `xtask/tests/greenfield_status.rs` fails CI on any disagreement. This track moves crate
boundaries, and several gated probes are defined by path or by crate:

| Probe | Reads | Defined over | Moved by |
|---|---|---|---|
| `ast_importers` | 7 | files in `bynk-emit/src` importing `bynk_syntax::ast` | folding `bynk-lower` into `bynk-emit` (§6 exit path). P7.12 moved `ir.rs`/`lower.rs` *out* of `bynk-emit` partly to lower this count. |
| `options_sources` | present | `struct CompileOptions` in `bynk-emit/src/project.rs` | moving `project` to the driver (§8) |
| `fs_below_driver` | 0 | files touching `std::fs` below the driver | moving `project` to the driver. Its `std::fs` uses sit in `project.rs`'s trailing test block, so they leave the counted set. |
| `emit_diagnostics` | `bynk-emit=14/19` | distinct `"bynk.*"` literals per crate `src/` | moving `project` to the driver. `project.rs` holds most of `bynk-emit`'s. |
| `ide_emit_edge` | absent | a `bynk-ide` → `bynk-emit` edge | must stay absent. §6 S1 adds `bynk-ide` → `bynk-lower`, which is allowed. |
| `unconsumed_ir_items` | 0 | public items of `bynk-ir`, owners `bynk-ir`/`bynk-lower` | renaming or folding the crate (§6) |

**Governing rules:**
- A slice that moves a probe re-points or re-stamps it (`cargo xtask greenfield-status --apply`)
  in the same commit as the change that moved it.
- No slice batches a probe-moving change with a probe-invariant one.
- A probe whose definition names a path this track moves is **re-pointed, not deleted**. The
  re-pointing is reviewed as part of the slice, with the probe's own doc comment updated to say
  why.

#1533 found the test-module-shape trap (its §3 A): a `#[cfg(test)] mod tests {` written on one line
is invisible to `test_mod_ranges`. That still applies to every module this track moves.

## 4. Requirements

- **H-1 Byte-identical output.** No slice changes emitted TypeScript, diagnostics, LSP responses,
  MCP renderings, OpenAPI documents or playground results. Each slice proves this with the
  existing tests, and adds a snapshot first where they are thin.
- **H-2 No new serial step.** A slice may shorten the build chain or leave it alone, never lengthen
  it.
- **H-3 Every edge carries use.** A declared dependency with no non-comment use in `src/` (or in
  `tests/` for a dev-dependency) is a defect. Workstream A adds a check for this (§5 A4).
- **H-4 Publish only what installs.** A crate is published only if an installed binary's normal
  dependency tree reaches it. Test and docs tooling is `publish = false`, with a path-only
  workspace entry, the way `bynk-testkit` is today.
- **H-5 Retired crates point onward.** A crate that leaves the published set gets a final release
  whose README names its replacement. It is not yanked.
- **H-6 Fail fast, never slower green.** A failing PR reports from the cheapest test tier that
  catches the failure, and no costlier tier holds it up. A passing PR's wall-clock is no longer
  than before the change. Workstream C proves both from CI timings (§7 C4).

## 5. Workstream A — dependency edges

Small, independent, probe-invariant. Each is a separate PR.

- **A1.** Remove `bynk`'s unused `bynk-render` dependency.
- **A2.** Take `bynk-lsp` off `bynk-emit` by moving `pred_condition_and_message` (and
  `pred_condition_and_message_for`) into `bynk-ts` (Q-A1).
  - It turns a `PredKind` into a `bynk_ts::TsExpr`, and `bynk-lsp` already depends on `bynk-ts`,
    so the move adds no edge.
  - Its helpers move with it: `escape_ts_string`, `escape_ts_literal` and the two
    `decimal_*_condition` functions. Escaping TypeScript literals belongs in the TypeScript crate
    anyway.
  - `bynk-emit` calls the moved functions from their new home. The one-call `bynk-ts` uses in
    `bynk-lsp` (`print_expr`) and `bynk-driver` (`print`) stay, because those are real uses of the
    printer.
- **A3.** Take `bynk-check` off `bynk-fmt` by moving the expression printer behind
  `expr_to_string` (the `to_string` family in `bynk-fmt/src/fmt.rs`) into `bynk-syntax` (Q-A2).
  `bynk-fmt` and the checker's three messages both call it there. A printer of the AST is
  something the syntax crate can own, and the checker keeps the exact text it renders today (H-1).
- **A4.** An xtask probe, `unused_deps`, that fails when a declared workspace dependency has no
  non-comment use (H-3) (Q-A3). Being a probe means it sits beside the other `greenfield-status`
  rows and needs no new tool under `deny.toml` or the MSRV policy. It reads 0 once A1–A3 land and
  is gated at 0.

## 6. Workstream B — the system model (`bynk-ir` becomes `bynk-model`)

### 6.1 The finding

`bynk-ir` has one consumer. Its 21 public items (16 types and 5 AST-walk helpers) describe a
program at the level of declarations:
- service protocols (`ProtocolIr`)
- handler kinds and routes (`IrHandlerKind`, `IrHttpMethod`, `CacheIr`)
- type shapes (`TypeShape`)
- store fields (`StoreFieldIr`)
- capability and attached-fn signatures (`OpSig`, `FnSig`)
- `given` references (`CapRefIr`)
- actor seams (`ActorSeamIr`)
- event subscriptions (`EventPatternIr`, `EventSubscriberShape`)

`bynk-lower`'s 17 entry points build them, and only `bynk-emit` reads them. Part 15.1 records that
this declaration-level IR is *adopted* and the expression-level IR is *refused*. This workstream
grows the adopted half and does not reopen the refused one.

The rest of the toolchain needs these facts, and each consumer gets them its own way:

| Model | Lives in | Consumers |
|---|---|---|
| `WireModel` (boundary types) | `bynk-check/src/wire.rs` | emitter codecs, `bynk_ide::wire_contract`, `bynk-lsp`, `bynk openapi`, `bynk mcp` |
| `ArchModel` (contexts, `consumes`, bindings) | `bynk-ide/src/architecture.rs` | the VS Code architecture map (via `bynk-lsp`), `bynk mcp` |
| `EntryHandler`/`EntryShape` (#2695) | `bynk-check/src/entry_signature.rs` | `bynk-emit` (`analyse_in_memory_project`), the playground via `bynk-wasm` |
| `ContextSequenceInfo` | `bynk-check/src/analysis.rs` | `bynk_ide::sequence` |
| Handler kinds, protocols, routes, shapes | `bynk-ir` | `bynk-emit` only |

`WireModel` is the pattern working. `bynk/src/openapi.rs` says "nothing here re-derives the
boundary … so the spec cannot drift" because one model feeds five consumers.

Handler kinds and protocols, meanwhile, are re-derived in each consumer. These are lines matching
AST variants outside `bynk-check` (which produces the facts) and `bynk-emit`/`bynk-lower` (which
already route through the IR):

| File | `HandlerKind::` | `ServiceProtocol::` | `HttpMethod::` |
|---|---|---|---|
| `bynk-ide/src/sequence.rs` | 8 | 0 | 0 |
| `bynk-ide/src/wire_contract.rs` | 9 | 0 | 1 |
| `bynk-ide/src/symbols.rs` | 8 | 8 | 0 |
| `bynk/src/openapi.rs` | 2 | 2 | 0 |
| `bynk-driver/src/smoke.rs` | 2 | 1 | 1 |

R6.16 (handler-invocation origin independence) gave the emitter one answer to "what is this
handler?" The editor, the OpenAPI generator and the smoke runner don't share it.

**The workstream:** make the crate the **system model**, the resolved description of what a Bynk
program *is* at the level of declarations. Every tool that needs those facts reads them from the
model, not from the AST: codegen, wrangler derivation, the editor's views, OpenAPI, MCP, the
playground and the smoke runner. `bynk-lower` stays the builder from a checked program to the model.

**What the model is not:**
- **An expression IR.** Bodies stay where Part 15.1 put them, and that entry's trigger is unchanged.
  A model item may *summarise* a body (`block_uses_emit`, `body_writes_state`). It never represents
  one.
- **The emit ABI.** ADR 0310's four items are about the emitted TypeScript. The model is a Rust-side
  description of the source program.
- **A second checker.** The model is built after `certify` and carries no diagnostics of its own.

### 6.2 Requirements

- **M-1 One source per fact.** A consumer that matches the AST for a fact the model carries is a
  defect.
- **M-2 Total over certified programs.** A gap is a missing variant, not a fallback. The existing
  `Ty::Unit` fallbacks in `bynk-lower` are documented exceptions to keep or retire, not a precedent.
- **M-3 Every item has a reader outside the model.** This is `unconsumed_ir_items` as it stands.
  B1 adds a second ratchet: the number of model items with a reader outside `bynk-emit` can only
  rise.
- **M-4 No new AST embedding.** The two documented ones (`TypeShape::Refined` carrying `BaseType`
  and `Refinement`, ADR 0366) stay. A new one needs the same written justification.

### 6.3 Questions

**Q1 — Where does the model sit? SETTLED: above `bynk-check` for B1–B3. B4 decides whether it moves below.**
- **(a) Above `bynk-check`, as now.** Every consumer already depends on the checker, so H-2 holds.
  But `WireModel` can't move in, because the checker's own boundary checks read it.
- **(b) Below `bynk-check`.** Every model gets one home, but `TyId` and the `actors` types would
  have to move down, or the model would use names. #1533's S7 (moving `block_uses_emit` off its
  `bynk-check` dependency) is the first step either way.

B1–B3 build under (a). B4 decides (b) on what B1–B3 showed: whether the remaining models
(`WireModel`, entry signatures) would read better in the same crate than they do in `bynk-check`.
If (b) is chosen, #1533's S7 is its first slice.

**Q2 — Is `bynk-lower` a separate crate? SETTLED: no. It merges into `bynk-model` as part of the
Q3 rename.** It is the model's only constructor. Under (a), merging it takes one step out of the
build chain (H-2 is improved, not just held). `unconsumed_ir_items`' two-owner rule (`bynk-ir` and
`bynk-lower` can't vouch for each other) becomes a module rule: items in `bynk-model`'s vocabulary
module need a reader outside the crate. The probe is re-pointed in the same PR (§3). If B4 chooses
(b), the builder splits back out, because something would then want the vocabulary without it.

**Q3 — Name. SETTLED: `bynk-model`, applied when B1 passes.** R10.1 says "a crate is named for what
it produces". `bynk-ir` reads as an intermediate representation for codegen, which is the reading
Part 15.1 narrowed away from. `bynk-model` sits beside the models §6.1 lists and still fits if
`bynk-lower` merges in. The crate docs say in one line that it is distinct from
`bynk_check::project_model`, which models units and manifests, not declarations.

The rename lands as B1's closing step, in its own PR that changes no behaviour, and takes in
`bynk-lower` (Q2). It publishes `bynk-model` as a new crate and gives `bynk-ir` and `bynk-lower` each
a final release (H-5). If B1 fails, the crates fold into `bynk-emit` and are never renamed.

**Q4 — Is the model a public interface? SETTLED: deferred until B3.** A serialised form (`bynk model --json`, or an MCP
resource) would give tools outside this repository one description of a project, for contract
diffs, architecture diffs in review and deploy planning. That would make the model's shape a
versioned contract. Recorded so no slice closes the option: model
items stay plain data, with no lifetimes and no references into the AST.

### 6.4 Slices and the exit criterion

| Slice | What |
|---|---|
| **B1 — the proof slice** | `bynk_ide::sequence`, `wire_contract` and `symbols` read `IrHandlerKind`/`IrHttpMethod`/`ProtocolIr` through `bynk-lower` instead of matching AST variants. Adds the M-3 ratchet. If it passes, it closes with the Q3 rename. |
| B2 | `bynk openapi` and `bynk-driver/src/smoke.rs` move onto the same items. |
| B3 | Entry signatures (#2695) become model items, read by `analyse_in_memory_project` and `bynk-wasm`. |
| B4 | Under Q1(a): `ArchModel`'s shapes move in, with `bynk_ide::architecture` keeping the queries. Under Q1(b): `WireModel` moves down too. |

**Exit criterion (B1).** The workstream continues if moving the three `bynk-ide` files onto the
model:
- removes the AST variant matches in §6.1 without adding equivalent IR-variant matches elsewhere in
  `bynk-ide`, and
- needs no new model item whose only reader is the IDE.

If B1 fails, the one-consumer shape is the honest one. `bynk-ir` and `bynk-lower` then fold into
`bynk-emit` as modules (`emit::decl`, `emit::lower`), removing two serial steps. That path **raises
the gated `ast_importers` count**, because `bynk-lower` imports the AST, and it has to re-argue that
probe's floor. That's the price of the fold, and the reason it isn't the default. Either outcome
beats today's: a separate crate whose separation buys nothing.

## 7. Workstream C — published set, tooling crates and CI tiers

- **C1.** Make `bynk-grammar` unpublished: `publish = false`, a path-only workspace entry, and out
  of both release workflows' crate lists.
- **C2.** Have `bynk-grammar` keep rule order itself instead of relying on `preserve_order`.
  Then remove `xtask/src/editor_contract.rs`'s key-sorting workaround.
- **C3.** Create one unpublished test crate, `bynk-tests`. It takes the `bynkc/tests` corpus,
  `bynk-testkit` and `bynk-grammar`. CI's `-p bynkc --test it` filters become `-p bynk-tests`.
  This is independent of §9 and can land first. It only moves files.
- **C4. Tiered CI testing.** Added on 3 November 2026 at the maintainer's request, for fast
  feedback and so that tests a failure has already made pointless don't hold up a failed merge.
  Measured on a green `main` run (`41574731024`): the "Test suite (ubuntu-latest)" job takes
  883s and is the critical path. Every other job finishes within 204s, in parallel. The job runs
  one `cargo nextest run --workspace` over every crate's unit tests, the integration corpus and
  the `workerd`/`wrangler`/`tsc` system tests together. The `ci` nextest profile sets
  `fail-fast = false`, so a broken unit test is reported only after the whole system suite has
  run. (Coverage is already off the PR path and the merge queue, so it's not part of this.)
  - **C4a. Measure.** Split the Test suite job's 883s into compile time and run time, and break
    the run time down by test group (nextest's JUnit output gives per-test durations). The
    numbers go in the slice proposal and pick C4c's shape. It has no dependencies and can start
    at once.
  - **C4b. Classify every test into exactly one tier.**
    - **Unit:** a crate's own tests, in its `src/` or its own `tests/`. The `bynk-tests` corpus
      tests that call a single crate's API move into that crate.
    - **Integration:** in-process pipeline tests and the fixture corpus, with no external
      process.
    - **System:** anything that starts the binary, `workerd`, `wrangler` or `tsc`.

    `bynk-tests` groups its modules by tier, and `.config/nextest.toml` names one filterset per
    tier. An xtask probe fails when a test matches no tier, or more than one. The docs and
    ADR-index consistency checks (`decisions_index`, `grammar_reference`, `readme_layering`,
    `cli_reference` and the like) aren't compiler tests, so they move to `xtask`, which the
    `drift` job already runs. Depends on C3.
  - **C4c. Gate the tiers.** The tiers run cheapest first. A failing tier stops the ones after
    it, and the rest of the job's work is cancelled. `fail-fast = false` stays within a tier, so a
    failing tier still reports all its failures. C4a's numbers pick between two shapes:
    - **Compile time dominates:** the tiers run as consecutive steps in the one job, so nothing
      compiles twice.
    - **The system tier's run time dominates:** it becomes a separate job, alongside the others,
      on a shared build. It is cancelled when an earlier tier fails.

    Either way, the merge queue and the three-OS matrix use the same order. H-6 is checked by
    comparing a green run and a seeded unit-test failure against C4a's baseline.

`bynk-wasm` stays in this repository, unpublished (Q-C1). It has two consumers, the playground and
the `vscode-bynk` web extension (ADRs 0517 and 0518). Both build from the release's
`bynk-web-<tag>.tar.gz` at a pinned tag, not from source: `vscode-bynk/scripts/fetch-web-assets.mjs`
says so, and so does the playground's `fetch-assets.sh`. The tarball is the contract, so moving
the crate elsewhere would only move who builds it. Its wasm32 CI build is also the only check that
`bynk-check`, `bynk-emit`, `bynk-strip`, `bynk-ide` and `bynk-fmt` still compile for the browser.

## 8. Workstream D — crate boundaries

- **D1.** Move `bynk_emit::project` into `bynk-driver`, the shared front end. `bynk-emit` then
  becomes code generation from a checked program.
  - First, the ~60 `emitter::` helpers `project.rs` uses (`ts_ident`, `extend_printed_at`,
    `wrangler`, `serialisation`, …) become a deliberate public API of `bynk-emit`, in its own PR.
  - `tests_emit.rs` (11,022 lines) splits into modules as it moves, under #1533's T1–T3 split
    tests and its no-orphan-context veto.
  - The output types (`ProjectOutput`, `Artefacts`, `Document`) stay in `bynk-emit`, because they are
    what code generation produces. Only the orchestration moves. `bynk-strip` reads nothing else
    from `bynk_emit::project` outside its tests, so it keeps depending on `bynk-emit` and not on the
    driver. `bynk-driver` then depends on `bynk-strip` (E1). If `bynk-strip` sat above the driver
    instead, `bynk` → `bynk-strip` → `bynk-driver` would put a new step on the build chain (H-2).
  - `bynk-wasm` switches to the driver for orchestration.
  - This moves `options_sources`, `fs_below_driver` and `emit_diagnostics` (§3).
- **D2.** Merge `bynk-render` into `bynk-driver` once §9 has removed `bynkc`. This amends ADR 0100.
  Two other users move with it:
  - **`bynk-render/tests/golden.rs`** and its `golden/` fixtures move into `bynk-driver`'s
    tests unchanged.
  - **`bynk-fmt`'s dev-dependency.** `roundtrip_error_renders_against_source_without_panicking`
    (`bynk-fmt/src/fmt.rs`, #735) renders the round-trip guard's diagnostic to prove its span
    stays inside the source. Keeping that test in place would need a `bynk-fmt` → `bynk-driver`
    dev-dependency. Cargo allows that cycle, but it's the kind of edge workstream A removes.
    Instead, the test moves to `bynk-driver`, which already depends on `bynk-fmt`. It drives
    the guard through `bynk-fmt`'s public API, because `roundtrip_error` is private, and keeps
    its assertion. `bynk-fmt`'s own test keeps the span-bounds check it already has
    (`err.span.end <= src.len()`), so `bynk-fmt` doesn't lose coverage.
- **D3.** `bynk-ir`/`bynk-lower`: decided by §6's exit criterion, not here.

## 9. Workstream E — installables

- **E1.** `bynk test` and `bynk compile` run the pipeline in-process, as ADR 0101's knock-on
  section already expected ("link `bynk-emit`'s test emission and run node").
  `bynk-driver::test_runner` holds the shared logic, so this replaces the three places that start
  a `bynkc` process.
  - `bynk compile --emit js` reaches `bynk-strip` today only through the `bynkc` it starts
    (`bynkc/src/main.rs`'s `EmitFormat::Js` arms). In-process, the driver calls
    `strip_project_to_js` itself, adding a `bynk-driver` → `bynk-strip` edge. So `bynk-strip`
    stays published under H-4 after E3: an installed binary reaches it.
- **E2.** Retire the compiler lookup (`BYNK_BYNKC` → `PATH` → sibling) and ADR 0434's skew gate,
  along with `bynk doctor`'s skew report. Nothing is left to skew against.
- **E3.** `bynkc` becomes a name the `bynk` executable answers to, for one release, and is then
  dropped before 1.0 (Q-E1).
  - `bynk` dispatches on `argv[0]`: run as `bynkc`, it accepts `bynkc`'s four subcommands with
    their current flags.
  - The `bynkc` crate stops existing as a separate binary. For that one release, the release still
    attaches a `bynkc-<target>` asset (a copy of `bynk`). The `bynkc` crate on crates.io gets a
    final version whose `bynkc` binary calls `bynk`'s entry point, so `cargo install bynkc` still
    works for that release (H-5). `vscode-bynk/scripts/fetch-toolchain.mjs` downloads
    `bynkc-<target>` by name, so that keeps a pinned extension release working.
  - The release after that drops the alias and the asset. That's the version `vscode-bynk` must
    have moved past (E4).
- **E4.** The language server is `bynk-lsp`, and the name `bynkc-lsp` is retired **before 1.0.0**
  (Q-E2). Today the crate is `bynk-lsp` but its binary is `bynkc-lsp` (`[[bin]] name`), so
  `cargo install bynk-lsp` installs a `bynkc-lsp`. E4 depends on nothing else in the track and can
  start at any time. It's a 1.0 blocker either way.

  Every place that carries the old name, measured at `104cd1494`:

  | Where | What changes |
  |---|---|
  | `bynk-lsp/Cargo.toml` `[[bin]] name` | The binary becomes `bynk-lsp`. |
  | `bynk-lsp/src/lib.rs` `SERVER_NAME`, `main.rs`/`lib.rs` crate docs | The server reports `bynk-lsp` as `serverInfo.name` in its `initialize` reply. |
  | `bynk-lsp/src/lib.rs` log messages (`bynkc-lsp: …`) | The prefix becomes `bynk-lsp:`. The log file is already `~/.bynk-lsp.log`. |
  | `.github/workflows/release.yml` (archive contents, loose assets) | Archives carry `bynk-lsp`, and the loose asset is `bynk-lsp-<target>`. |
  | `scripts/check-installer-naming.sh` | The guarded strings follow `release.yml`. |
  | `scripts/install.sh`, `scripts/install.ps1` | They install `bynk-lsp`. |
  | `features/src/bynk/` (`install.sh`, `devcontainer-feature.json`), `features/test/bynk/*.sh` | The devcontainer feature installs `bynk-lsp`. |
  | `.github/workflows/ci.yml` installer smoke test | It checks for `bynk-lsp`. |
  | `bynk doctor` editor-support check, `bynk/tests/doctor.rs`, `bynk/tests/golden/doctor.{short,json}` | `doctor` looks for and names `bynk-lsp`. |
  | `vscode-bynk`: `fetch-toolchain.mjs`, the `bynk.executablePath` default | The extension downloads and defaults to `bynk-lsp` (its own repository). |
  | Site (about 10 pages, including `editor-support.md`'s generic-LSP setup and `bynk-lsp.md`), `README.md`, `SECURITY.md`, crate READMEs | The docs say `bynk-lsp`. `bynk-lsp.md` drops its "the crate builds the `bynkc-lsp` binary" explanation. |
  | `design/` (ADRs, specs, roadmaps) | Left as written. ADRs record what was true when they were decided. The front-loaded installables ADR (§13) records the rename. |

  Two steps:
  - **E4a — rename, with one release of overlap.**
    - Everything in the table switches to `bynk-lsp` in one PR. The release still ships the old
      name next to the new one. Archives, the install scripts and the devcontainer feature put a
      `bynkc-lsp` copy beside `bynk-lsp`, and the release attaches both
      `bynk-lsp-<target>` and `bynkc-lsp-<target>`.
    - The overlap reaches the installers, not just the release assets, because users' own editor
      setups (Neovim, Helix, any generic LSP client with `cmd = "bynkc-lsp"`) can't be updated for
      them. The release notes name the change and the one-line config fix.
    - `vscode-bynk` ships a release that looks for `bynk-lsp` first and falls back to
      `bynkc-lsp`, and stops downloading `bynkc`.
  - **E4b — drop the old name.** The next release removes the `bynkc-lsp` copy, asset and
    installer entries. Only installer lines and release assets change. If E3 is ready by then, its
    alias removal lands in the same release, so `vscode-bynk` makes one move. If it isn't, E4b
    doesn't wait for it.
- **E5.** Change the emitted header `// Generated by bynkc` and re-bless the fixtures. Do this
  separately from everything else in the workstream so the diff is only that header.

## 10. Not in this track

Each item names what would bring it back.

- **Splitting the wire/contract cluster out of `bynk-check`** (`wire`, `wire_format`, `contract`,
  `contract_compat`, `schema_registry`, ~9.4k lines). It calls `symbols` and `resolver`, so it
  needs the core checker types split out first. *Trigger:* §6 Q1(b) lands.
- **Splitting the checker core** (`checker`, `symbols`, `resolver`, `project_model`, with
  in-degrees of 30–57). There's no clean line to draw.
- **A registry-client or deploy crate out of `bynk`** (`registry` + `fetch`, ~7k lines with the
  TLS stack; `deploy`, 15.8k). It adds a crate. *Trigger:* `bynk`'s own compile time, measured, is
  the bottleneck after §9.
- **Moving `bynk-wasm` to its own repository.** See §7.
- **Unbundling `bynk.cloudflare` (L3) to the registry.** That belongs to the standard-library
  track (its D9) and the packaging track.

## 11. Settled questions

All settled 2 November 2026, in the settling review of #2776, except Q-C2, which the maintainer
added and settled on 3 November 2026.

| Question | Answer | Where |
|---|---|---|
| **Q-A1** Where `pred_condition_and_message` lands | `bynk-ts`, with its escaping and decimal helpers | §5 A2 |
| **Q-A2** How the checker stops depending on `bynk-fmt` | The expression printer moves into `bynk-syntax` | §5 A3 |
| **Q-A3** How H-3 is enforced | An xtask probe, `unused_deps`, gated at 0 | §5 A4 |
| **Q-C1** Whether `bynk-wasm` stays | Stays. Both consumers build from the `bynk-web-<tag>.tar.gz` release asset | §7 |
| **Q-C2** How CI runs the tests (added 3 November 2026) | Three tiers (unit, integration, system), each gated on the one before, without lengthening the green path. C4a's measurements pick whether the system tier is its own job | §7 C4 |
| **Q-E1** What happens to `bynkc` | An `argv[0]` alias of `bynk` (plus a `bynkc-<target>` asset) for one release, then dropped before 1.0 | §9 E3 |
| **Q-E2** How the language server becomes `bynk-lsp` | `bynk-lsp` is the name, and `bynkc-lsp` is retired before 1.0.0. One release ships both names (assets *and* installers), `vscode-bynk` prefers `bynk-lsp`, then the old name drops | §9 E4 |
| **§6 Q1** Where the model sits | Above `bynk-check` for B1–B3. B4 decides whether to move below | §6.3 |
| **§6 Q2** Whether `bynk-lower` stays separate | No, it merges into `bynk-model` with the rename | §6.3 |
| **§6 Q3** Name | `bynk-model`, applied when B1 passes | §6.3 |
| **§6 Q4** Public interface | Deferred until B3 | §6.3 |

## 12. Candidate slice decomposition

Provisional. Each slice becomes an increment proposal, filed as a sub-issue of the spine.

| Order | Slice | Depends on | Probe-moving |
|---|---|---|---|
| 1 | A1, A2, A3 (separate PRs) | — | no |
| 1 | C1, C2 | — | no |
| 1 | C4a | — | no |
| 1 | B1 (+ Q2/Q3 rename and merge if it passes) | the system-model ADR (§13) | `unconsumed_ir_items` (re-pointed by the rename) |
| 2 | A4 | A1–A3 | adds `unused_deps` |
| 2 | C3 | — | no |
| 3 | C4b, then C4c | C3, C4a | no |
| 2 | B2, B3 | B1 passed | no |
| 3 | D1 (public `emitter` API, then the move) | — | `options_sources`, `fs_below_driver`, `emit_diagnostics` |
| 4 | E1, E2 | D1 | no |
| 1 | E4a (then E4b one release later) | — | no |
| 5 | E3, D2 | E1, E2 | no |
| 6 | E5 | E3 | no |
| — | B4 | B3 | depends on whether B4 chooses (b) |

The release workflows' hardcoded crate lists change in every slice that adds, removes or renames
a crate: C1, the Q3 rename, D2, E3 and the §6 fold path.

## 13. Front-loaded ADRs

- **The installables.** `bynk`, plus optionally `bynk-lsp`, are the only release binaries.
  `bynk` runs every command in-process. The language server is `bynk-lsp`, and `bynkc-lsp` is
  retired before 1.0.0. This amends ADR 0101 and retires ADR 0434's gate. It lands before E1 or
  E4a, whichever comes first.
- **The system model's scope and placement.** §6 Q1–Q3, and amends Part 15.1's IR entry. Lands
  before B1.
- **The published set.** H-4 and H-5. This amends ADR 0099's naming of `bynk-grammar` as the model
  leaf and ADR 0100's `bynk-render`. Lands before C1.

## 14. Non-goals

- **The expression IR**, refused in Part 15.1 with its own trigger.
- **Changing emitted output, diagnostics or editor behaviour** (H-1). E5's header change is the one
  exception, and it is isolated.
- **Comment rewrites, test-density targets and general module splits.** See §1.1. C4 moves and
  classifies existing tests; it doesn't add tests or set coverage targets.
- **New crates for build-time reasons alone.** See §10.
