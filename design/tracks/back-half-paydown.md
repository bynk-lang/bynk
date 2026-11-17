# Back-half paydown — a feature freeze to pay down the code generator, driver and deploy debt

- **Status:** Settled. The §9 questions were closed with the maintainer on 17 November 2026.
  Phase 0 is complete (P0.1–P0.5; P0.6 was retired into 1B.3). Phase 1 is next. Spine
  bynk-lang/bynk#3676.
- **The freeze:** the maintainer paused new feature development on `bynk` from v0.639.1. §2 sets
  out its terms, as settled by Q1, and the freeze ADR added with this settling records them and
  the lifting condition.
- **Evidence:** [`../reviews/2026-11-17-back-half-debt-review.md`](../reviews/2026-11-17-back-half-debt-review.md).
  Section numbers below that start with "review §" refer to it.
- **Theme:** internal architecture, not language surface. It continues the precedent of
  `compiler-architecture.md`, `identity-and-totality.md` and `content-ownership.md` (all retired),
  under the principles of [`../bynk-greenfield-compiler.md`](../bynk-greenfield-compiler.md):
  - P5, "when the right shape lands, the wrong one is deleted in the same change";
  - R2.12, the wildcard-arm rule;
  - Part 15.1's expression-IR entry, which refuses a second expression code generator but
    explicitly leaves open "the tree-native route (lowering to `bynk_ts` nodes, retiring
    `ts_writes`)". Phase 1A is that route.
- **Posture:** Feature track per [ADR 0076](../decisions/0076-feature-track-posture.md). It
  qualifies on two axes: it spans many increments, and its shape is not yet settled (§9).
- **Not a build authorisation.** Each slice below is cut as an ordinary increment proposal, a
  sub-issue of the spine.

---

## 1. The theme

Since 1 October the back half of the toolchain has roughly tripled. Production code in
`bynk-emit`, `bynk-driver`, `bynk-ts`, `bynk-model` and the CLI went from about 37k lines to about
110k. Structural paydown has not kept pace (review, Headline), and the gauges built to catch that
have drifted:

- gated probes are re-baselined rather than enforced;
- the trend rows have gone stale;
- the probes' scope misses the growth.

This track pays the debt down while nothing else is moving underneath it.

**What it settles:**

- **(a)** The terms of the freeze (§2).
- **(b)** The invariants every slice keeps (§3).
- **(c)** Four phases, in order, with their slices (§4–§7):
  - Phase 0 restores the gauges;
  - Phase 1 removes the structural debt;
  - Phase 2 splits modules along the seams Phase 1 creates;
  - Phase 3 rebalances the tests.
- **(d)** Exit criteria stated as probe readings, not dates (§8).

**What it does not settle:** any change to the language. The core language is treated as stable
for the track's duration. Language *design* (proposals, track docs in settling) may continue; it
is not *built* until the freeze lifts.

---

## 2. The freeze

From v0.639.1 until this track's Phase 1 exit criteria are met (§8):

**Allowed on `main`:**

- **This track's slices.**
- **Defect fixes.** Each carries a test that fails without it, ideally at unit level. A fix to a
  documented feature that does not work counts, provided it adds no syntax or API (Q1).
- **Security fixes and advisories.**
- **Dependency bumps.**
- **CI, release and documentation changes.**
- **Track retirements and status updates** for tracks already built, for example
  `model-diagrams.md` §16.

**Not accepted for build (no `accepted` label):**

- new language surface;
- new standard-library APIs;
- new deploy resource kinds or binding kinds;
- new build targets or service protocols;
- new CLI subcommands.

This applies to `vscode-bynk` too, wherever a change would need a new compiler surface.
Extension work that needs no compiler change, such as polish and fixes, is allowed (Q1). A
`bynk-std` addition is not allowed, even one that needs no emitter change: one rule is easier to
hold than a judgement call per PR (Q1).

**Lifting the freeze.** The freeze lifts when Phase 1's exit criteria are met. Phases 2 and 3
then run alongside resumed feature work. From then on the Phase 0 ratchets hold the line: a
feature that needs a probe to rise must argue for it in review, as the function-length ratchet
already requires.

---

## 3. Invariants

Every slice keeps all of these:

1. **No capability is dropped.** Every construct that compiles at v0.639.1 compiles at every slice
   boundary, with the same semantics. This is the trajectory's §2 invariant, reapplied.
2. **Emitted behaviour is unchanged.** Golden output may change *textually* only in the slices
   that move a region from strings to `bynk_ts` nodes, where the printer's layout replaces
   hand-formatted text.
   - Such a slice states that its golden diffs are layout-only.
   - It proves this with `tsc_verify` and the run-fixture suite, both unchanged.
   - Behavioural golden changes ride a defect fix, never a paydown slice.
3. **The old shape is deleted in the same slice** (P5). A migration may use a temporary bridge, for
   example a `LoweredExpr::Text` variant. The bridge is counted by a probe that can only fall, and
   it is gone when its phase exits.
4. **No probe moves the wrong way.** Phase 0 makes this mechanical. Until then, a slice that raises
   `ts_writes`, `ts_raw_sites`, `ts_text_idents`, `lowering_text_bridge`, `ts_any`,
   `emit_abi_shapes`, `verbatim_sites`, `span_keyed_maps` or `wildcard_arms` must say why in its PR description. For the growth probes
   that is a `raises:` line. A Phase 1 slice that converts raw statements into nodes may
   legitimately raise `ts_writes` in the short term (retiring a raw string can add `format!`
   lines). It uses `raises:` with a reason such as "replaces N raw statements", and the
   `ts_raw_sites` fall in the same PR shows the trade.
5. **The integration corpus is not thinned before Phase 3.** It is the safety net for Phases 1
   and 2.

---

## 4. Phase 0 — make the gauges hold

Small, mostly in `xtask`. It lands first because every later slice is measured by it.

| Slice | Change | Closes (review) |
|---|---|---|
| **P0.1** | Turn `ts_writes`, `ts_any`, `emit_abi_shapes`, `verbatim_sites` and `span_keyed_maps` from "differs from committed" gates into **ratchets**, the `fn-lengths` model: a fall may be applied freely; a rise fails unless the committed ceiling is raised in the same PR, where review sees it. | §1.1 |
| **P0.2** | Count the raw-text escapes the TypeScript probes miss. Add two gated growth probes: `ts_raw_sites` (`TsStmt::raw(`) and `ts_text_idents` (`TsExpr::Ident(format!(`). `ts_writes`, `ts_any` and `verbatim_*` already scan `bynk-driver/src/project/**`, including `tests_emit/`, via `codegen_family_files` (#2982), so no widening is needed (#3681). | §1.3 |
| **P0.3** | Refresh trend rows at stamp time (`cargo xtask stamp --apply` regenerates the whole table), and drop the deleted crates from `test_density`. | §1.2 |
| **P0.4** | Opt `bynk-ts` (8 wildcard arms) and `bynk-model` (5) into `[lints] workspace = true`, after fixing their arms. | §1.2 |
| **P0.5** | Add a `module_lines` probe: the largest source file per crate, and files at or above 2,000 lines. Report only (trend) in this phase; it becomes a ratchet in Phase 2. | §6.1 |
| ~~**P0.6**~~ | **Retired into 1B.3** (#3689). The premise, that these files were unchecked, was wrong. The test-runtime fragments are type-checked under `tsc --strict` in context, because `bynk test` compiles each generated test module and fails on a `tsc` error. `http_client_runtime.ts` is checked by `system::http_client`. The residual gaps belong to 1B.3: the fragments aren't standalone modules, and a fragment no fixture includes goes unchecked. | §3 (corrected) |

**Exit:** the ratchets are live, the trend rows are current, and `bynk-ts` and `bynk-model` deny
wildcard arms.

---

## 5. Phase 1 — structural debt

Four strands. **1A** and **1D** are independent and can run in parallel. **1B** depends on 1A's
first slices only in so far as both edit `bynk-emit`. **1C** should follow 1B.1, so that the
binding model has one compose path to serve.

### 1A. Tree-native lowering (review §2.1, §2.2, §2.4)

| Slice | Change |
|---|---|
| **1A.1** | Move `LowerCtx`, `ModuleCtx` and type rendering out of `emitter.rs` into `emitter/ctx.rs`, `emitter/lower_ctx.rs` and `emitter/ts_types.rs` (a mechanical move, allowed early; see §6). Byte-identical goldens, no re-exports. |
| **1A.2** | Introduce the bridge: `enum LoweredExpr { Node(TsExpr), Text(String) }`, `enum LoweredStmt { Node(TsStmt), Text(String) }` and `Lowered { pre: Vec<LoweredStmt>, expr: LoweredExpr }`. `Pre::lower` and `Pre::absorb` keep returning text for unconverted callers (the one place a node is printed); `Pre::lower_node` and `Pre::absorb_node` return the bridge value. Count the text side with the gated growth probe `lowering_text_bridge`. Convert the first family, literals (`IntLit`, `FloatLit`, `DecimalLit`, `DurationLit`, `StrLit`, `BoolLit`, `UnitLit`, `None`; not `InterpStr`), to prove the path. `ts_ty` keeps its signature: each family slice that converts a type-annotated construct switches its own call sites to `ts_ty_to_ts_type` instead. |
| **1A.3** | Wrappers and simple composites: `Paren`, both `UnaryOp` arms (plain `-`/`!` and the Decimal-negation call), `Ok` (with the `HttpResult` overload), `Err`, `Some`, `ListLit`, and `Wire` (pass-through). Adds `TsUnaryOp::Neg` to `bynk_ts`, which prints `-(-x)` rather than `--x`. Runtime-constructor calls are call nodes over genuine identifiers (`HttpResult.Ok` a member access). **The 1A convention (Decision A):** a converted parent builds a node only if *every* child is a node; otherwise it renders its children through `LoweredExpr::into_text` and returns text exactly as before, through one shared helper (`node_or_text`). A text child is never wrapped in `VerbatimExpr` or an `Ident` carrying expression text, so `verbatim_sites`, `ts_text_idents` and `ts_raw_sites` never rise for it. Node coverage grows as each family converts. |
| **1A.4–n** | Convert the remaining lowerings to `Node`, leaves first (literals are done in 1A.2, wrappers in 1A.3): identifiers, operators, calls, then control flow and the list kernel. One expression family per slice, each following the 1A.3 mixed-children rule. Each slice lowers `ts_writes`; `lowering_text_bridge` falls as text construction sites are replaced. |
| **1A.x** | Once nothing produces `Text`, delete the variant and `print_expr` round-trips. `ts_array_elem`-style text inspection goes too; the printer owns precedence. |
| **1A.imports** | Derive each module's runtime and type imports from the built tree using `bynk-ts/src/name_closure.rs`. Delete the `*needs*` / `*mentions*` / `*uses*` prediction functions. |
| **1A.fallback** | One policy for a missing checker fact: an internal-compiler-error diagnostic, never `any` and never `undefined`. Remove the `"any"` fallbacks. |
| **1A.resolve** | The checker records the resolved callee and store kind on the typed tree. Emit stops comparing names against string literals (for example `cap == "Idempotency"`, the store-head match in `emit.rs`). |
| **1A.state** | Remove the two `thread_local!`s (`FAULT_SOURCE`, `type_renames::SPELLINGS`) by passing module context explicitly. |
| **1A.lints** | Bring `bynk-emit` to zero wildcard arms (192 at v0.639.1, review §1.2) and opt it into `[lints] workspace = true`, which is a Phase 1 exit criterion. Until it lands, a slice that adds an arm to `bynk-emit` says why (invariant 4). Each 1A slice removes the arms in the code it rewrites; this slice owns whatever remains, chiefly over `Ty`, `BodyMode` and `TypeRef` outside lowering. |

### 1B. One compose model; code generation out of the driver (review §2.3, §3)

| Slice | Change |
|---|---|
| **1B.1** | A target-neutral composition model, `ComposePlan`: units, their dependencies, event subscriptions with the envelope and isolation rules, and bindings. Both `emit_composition_root` (Bundle) and `emit_worker_compose` (Workers) render from it. The envelope and failure-isolation rules then exist once. |
| **1B.2** | Move test-harness generation (`project/tests_emit/*`), `emit_composition_root` and the per-Worker emission out of `bynk-driver` and into a `bynk-emit::testgen` module (Q3). The driver keeps orchestration, file I/O and process spawning. This removes `bynk-emit`'s dev-dependency on `bynk-driver`. |
| **1B.3** | Turn the test runtime into real, type-checked modules under `bynk-emit/runtime/src/testing/`. Move the `makeIntegration*` / `makeTestState` fakes out of production `storage.ts` into it. Delete the driver's TS lexer (`names_ident`) and its text patching of emitted TypeScript (`merge_cookie`). Bring `http_client_runtime.ts` under the runtime's `tsconfig.json` and `bundle.mjs --check`, and add a guard that every test-runtime module is type-checked whether or not a fixture uses it (absorbs the retired P0.6). |
| **1B.4** | Deduplicate the shared node builders (`str_lit`, `ident`, `member`, `call`) and `append_missing_bindings` (three copies) into one `bynk-emit` module. |

### 1C. One binding model (review §4)

| Slice | Change |
|---|---|
| **1C.1** | A typed `WorkerBindings` model, owned by `bynk-emit` and serialisable. The driver's duplicate-binding check consumes it directly instead of re-deriving it. |
| **1C.2** | `wrangler.rs` renders TOML from the model, including the `[env.*]` blocks. The CLI's `synthesise_environment_block` and its hand-written table list are deleted. |
| **1C.3** | Replace the CLI's parallel serde `WranglerConfig` (13 binding structs) with the shared model. Fold the six sidecar JSON manifests into it where they describe the same Worker. |

### 1D. Deploy (review §5)

| Slice | Change |
|---|---|
| **1D.1** | A `ResourceKind` abstraction. Each kind provides its parse-from-bindings, plan line, ledger record, orphan rule, provision and revert. The ledger stores kinds uniformly, with a migration that reads the current per-kind maps. `find_orphans` becomes one loop. Converted one kind per slice, KV first. |
| **1D.2** | Apply executes the `Plan`: `derive_plan` produces typed steps, and `deploy_one` takes the steps, not `Resources`. The ledger is written per completed step, at one site. `plan.rs::run` shrinks to orchestration. |
| **1D.3** | One Cloudflare client layer. A `Wrangler` trait with a recording fake (removing the on-disk `#!/bin/sh` fakes and their `cfg(unix)` gates). A REST client moved out of `migrate::live` into a shared module, constructed once. Typed errors replacing `Result<_, String>`. Retry with backoff for idempotent calls, and shared pagination. |
| **1D.4** | Move account-resource management to REST, as Q4 settled. Resource create, read and delete moves kind by kind behind the 1D.3 trait, starting with KV. `deploy`, `dev`, `tail`, D1 migrations and `rollback` stay on wrangler. Add a `bynk doctor` check that `CLOUDFLARE_API_TOKEN` is set when resource management needs it. The ADR recording the boundary lands with 1D.3. |
| **1D.5** | One module for external-tool versions and spawning: wrangler, npm, tsc, git. The `wrangler@4` pin lives in one place. The parallel `npx` flake is fixed by pre-warming or serialising the wrangler fetch. The `BYNK_*` environment-variable protocol becomes a single generated constant table shared by Rust and TypeScript. |

---

## 6. Phase 2 — module splitting

Split along the seams Phase 1 creates, not by line count. Splitting `lower.rs` before 1A would
mostly be redone by 1A.

**Exception.** Purely mechanical moves, with no logic change, may land earlier where they make a
Phase 1 slice smaller. 1A.1's move of `LowerCtx` out of `emitter.rs` is the example. While
feature work is frozen, such moves cost no merge conflicts.

**Targets.** The structure below is suggested; each slice proposes its own:

- `emit.rs`, split by declaration kind: services, agents (state record, persistence, dispatch),
  free functions, WebSocket dispatch.
- `lower.rs`, split by expression family, following 1A's slicing.
- `emitter.rs`, split into headers, imports, boundary helpers and the project driver.
- `workers_entry.rs`, split per handler kind: fetch, scheduled, queue, email, WebSocket.
- `bynk-driver/src/project.rs`, which becomes what remains after 1B.2.
- `bynk/src/deploy/plan.rs`, which becomes what remains after 1D.2, with orchestration separated
  from rendering.

**Ratchet.** From settling onwards, a Phase 1 slice must not push a file newly across 2,000
production lines; if it does, its PR says why, as invariant 4 requires for probe rises. 1A and
1B reshape the largest files, so this rule applies before the mechanism exists. P0.5's
`module_lines` then becomes a mechanical ratchet when this phase starts. A file over the
threshold may not grow, and no file may newly cross it. Settled by Q5:

- the measure is `module_lines`' production lines;
- no file may newly cross **2,000** lines;
- a file already over 2,000 gets a recorded ceiling that can only fall, as `fn-length-ceilings.txt`
  works for functions;
- the track-exit target is that no back-half file exceeds **3,000** lines.

`bynk-ts/src/printer.rs` is in scope: it is large, but it is the best-tested file in the back
half, so splitting it is low-risk.

---

## 7. Phase 3 — rebalance the tests

Begins as each Phase 1 and Phase 2 seam lands. In practice it overlaps Phase 2: a module split
out carries its unit tests with it.

| Slice | Change |
|---|---|
| **3.1** | Unit tests at each new seam: a lowering returns a `TsExpr` to assert on, a `ComposePlan` decision, a `WorkerBindings` derivation, and a `ResourceKind`'s plan and apply against the 1D.3 fakes. |
| **3.2** | Re-measure coverage per crate with `cargo-llvm-cov`, as the [9 October review](../reviews/2026-10-09-unit-test-coverage-review.md) did, for own versus full coverage. Record the result as a new dated review. |
| **3.3** | Thin the corpus. A fixture whose only assertion is now made by a unit test is retired. Goldens remain for end-to-end shape, `tsc_verify` coverage and a curated set of behavioural run fixtures. Retirement follows Q6's three conditions (§9). |
| **3.4** | Per-crate floors on own-unit-test density (or own coverage, if 3.2 makes that cheap enough to gate), ratcheted. |

---

## 8. Exit criteria

| Measure | Today (v0.639.1) | Phase 1 exit (freeze lifts) | Track exit |
|---|---:|---:|---:|
| Gated growth probes | re-baselined (165 rises of `ts_writes`) | ratchets (P0.1) | ratchets |
| `ts_writes` (bynk-emit + driver `project/**`) | 1,502 | ≤ 40% of today | ≤ 20% |
| `ts_raw_sites`: `TsStmt::raw(` sites (emit + driver) | 99 | ≤ 10, each with a stated foreign-text reason | ≤ 5 |
| `ts_text_idents`: `Ident(format!(` sites (emit + driver) | 14 | 0 | 0 |
| `lowering_text_bridge`: text construction sites of the lowering bridge | 127 (at 1A.2) | 0: lowering returns `TsExpr` only; `Text` bridge deleted | 0 |
| Import-prediction functions | 17 | 0 | 0 |
| `"any"` fallbacks in lowering | 5 | 0 | 0 |
| Compose implementations | 2 | 1 (`ComposePlan`) | 1 |
| Places a binding kind is defined | ≥ 3 | 1 (`WorkerBindings`) | 1 |
| Deploy: apply consumes `Plan` | no | yes | yes |
| Deploy: Cloudflare client layers | 2, untyped | 1 trait, typed errors | 1 |
| `bynk-emit` dev-dependency on `bynk-driver` | yes | no | no |
| Crates denying wildcard arms | 2 (bynk-ts, bynk-model) | bynk-ts, bynk-model, bynk-emit | + bynk-driver, bynk |
| Back-half files ≥ 5,000 lines | 6 | — | 0 |
| Back-half files ≥ 3,000 lines (production) | per `module_lines` | — | 0 (Q5) |
| Files newly crossing 2,000 production lines | — | — | 0, by ratchet (Q5) |
| bynk-emit unit tests per 1k production lines | 7.1 | — | per 3.4's floor |

The `ts_writes`, `ts_raw_sites` and `ts_text_idents` targets are measured over bynk-emit and
bynk-driver's `project/**` *combined* (`codegen_family_files`). 1B.2 moves `tests_emit` from the driver into bynk-emit, so the move itself
is neither progress nor regression.

The back-half file rows count `src/` files in `bynk-model`, `bynk-emit`, `bynk-ts`, `bynk-driver`,
`bynk-strip` and `bynk`. On 1 October they read 5 and 10. Review §6.1's 18 and 50 count the whole
workspace.

The numeric targets (`ts_writes` ≤ 40% of the P0.2 baseline at Phase 1 exit and ≤ 20% at track exit; production `TsStmt::raw` sites ≤ 10 and ≤ 5) were adopted with the maintainer at settling, on 17 November 2026. They are part of the lifting condition the freeze ADR names. The other rows are structural yes/no.

---

## 9. Settling questions (all resolved)

Closed with the maintainer on 17 November 2026.

- **Q1. Freeze scope.** §2 stands as written, with three clarifications, now written into §2:
  - a fix to a documented feature that does not work is a defect fix, provided it adds no syntax
    or API;
  - no `bynk-std` additions, even ones that need no emitter change;
  - `vscode-bynk` work that needs no compiler change is allowed.
- **Q2. Golden churn in 1A.** Layout-only golden diffs are accepted. Each 1A PR states how many
  goldens changed, shows representative diffs, and keeps `tsc_verify` and the run fixtures
  unchanged and green. 1A does not first make the printer reproduce today's hand formatting:
  that would pin the string-built layout in the goldens. Each 1A slice covers one expression
  family, so its golden diff stays reviewable.
- **Q3. Where test generation lives.** A `bynk-emit::testgen` module. A new crate would add a
  publish-list entry, a version and a dependency edge for code only the emitter calls. Promote it
  to a crate only if compile times measurably need it.
- **Q4. Wrangler or REST for resource operations.** A deliberate hybrid for this track, with the
  boundary recorded by an ADR when 1D.3 lands:
  - **Wrangler** keeps building and running code: `deploy` (it bundles npm dependencies with
    esbuild before uploading; the emitted `wrangler.toml` sets no `no_bundle`), `dev` (local
    workerd), `tail`, D1 migrations (`wrangler d1 migrations apply` tracks applied migrations in
    its own table) and `rollback`.
  - **REST** takes creating, reading and deleting account resources (KV, D1 databases, R2,
    queues, Vectorize, secrets, triggers, and listing deployments), behind 1D.3's client trait,
    one kind at a time, starting with KV. This retires the matching on wrangler's error text,
    gives a fake without on-disk shell scripts, and centralises retries and pagination.
  - **Credentials.** `wrangler login` keeps working for everything wrangler still does. Resource
    management requires `CLOUDFLARE_API_TOKEN`, and `bynk doctor` says so up front rather than
    leaving it to be found mid-deploy.
  - The longer-term direction this boundary leaves room for is recorded in §12.
- **Q5. The module-size threshold.** A ratchet on `module_lines` production lines: no file may
  newly cross 2,000 lines; existing files over 2,000 get ceilings that can only fall; the
  track-exit target is no back-half file over 3,000. Written into §6 and §8.
- **Q6. Corpus-thinning criteria for 3.3.** A fixture is retired only when all three hold:
  1. the retiring PR names the unit test that now makes the same assertion;
  2. the `diagnostic_coverage` gate does not move, so every diagnostic code stays asserted;
  3. the fixture is not a run fixture or a `tsc_verify` golden, which stay as end-to-end
     coverage.

  The `fixture_kinds` trend row shows progress.
- **Q7. ADR posture.**
  - The freeze and its lifting condition: now, with this settling, because the freeze is already
    in force.
  - Tree-native lowering: with the first 1A slice.
  - Where test generation lives (Q3): with 1B.2.
  - The wrangler/REST boundary (Q4): with 1D.3.
  - The test runtime is first-party runtime code: with 1B.3, which absorbed the retired P0.6.
  - The growth ratchet: already recorded, with P0.1.

---

## 10. Front-loaded ADR candidates

- **Gated growth probes are ratchets.** This changes the meaning of "gated" set by #999,
  ADR 0389 and ADR 0390. It is load-bearing for every later slice.
- **The feature freeze and its lifting condition** (§2). Recorded so that the lift is a decision,
  not a drift.
- **Tree-native lowering.** It records that 1A is the route Part 15.1 left open, so that the
  expression-IR refusal's trigger is not mistaken as having fired.
- **The test runtime is first-party runtime code,** type-checked like the rest (1B.3).

Numbers are assigned at merge by the stamp, as usual.

---

## 11. Non-goals

- **No expression IR.** Part 15.1's refusal stands. 1A lowers into the existing `bynk_ts` tree.
- **No change to emitted behaviour,** beyond defect fixes that ride their own PRs (§3).
- **No new deploy resource kinds, targets or protocols** during the freeze, even ones that 1D.1
  would make easy.
- **No incremental-compilation scheduler** (R3.15 remains deferred).

---

## 12. Beyond this track

Recorded so that Phase 1 does not close off a direction the maintainer is considering. None of
this is built under this track or during the freeze.

- **First-party bindings carry their platform-integration code** as well as their runtime code.
  1D.1's `ResourceKind` is shaped for that move: each kind takes what it needs through the trait
  and reaches into no CLI internals. A later track could then let a first-party binding supply its
  own kind, so adding a resource kind becomes a binding change rather than a CLI change. 1D.3's
  REST client is what makes this possible, since a binding can describe typed API calls but not
  wrangler invocations.
- **bynk owns a bundling step.** `wrangler deploy` and `wrangler dev` currently bundle for us.
  A bynk-owned bundle (emitted TypeScript stripped by `bynk-strip`, plus npm dependencies) is the
  hinge for the two moves below.
- **Miniflare for `bynk dev`.** It gives programmatic binding configuration, faster starts, and
  no `npx wrangler` fetch at dev time. It depends on the bundling step.
- **REST script upload for `bynk deploy`.** It makes wrangler optional. It also depends on the
  bundling step.

That later track's ADRs would supersede Q4's interim boundary.
