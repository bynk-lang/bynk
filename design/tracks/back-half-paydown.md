# Back-half paydown — a feature freeze to pay down the code generator, driver and deploy debt

- **Status:** Settling. The spine issue is not yet opened.
- **The freeze:** the maintainer has paused new feature development on `bynk` from v0.639.1, and
  the pause is in force now. §2 sets out its proposed terms. Their exact scope (Q1) and the ADR
  that records them (§10) are still being settled. Until they are, a change that §2 would exclude
  is held for the maintainer's decision, not built.
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
- **Defect fixes.** Each carries a test that fails without it, ideally at unit level.
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
   `ts_writes`, `ts_raw_sites`, `ts_text_idents`, `ts_any`, `emit_abi_shapes`, `verbatim_sites`,
   `span_keyed_maps` or `wildcard_arms` must say why in its PR description. For the growth probes
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
| **1A.1** | Introduce the bridge `enum LoweredExpr { Node(TsExpr), Text(String) }` and `pre: Vec<TsStmt>`. Make `ts_ty` return `TsType`. Move `LowerCtx`, `ModuleCtx` and type rendering out of `emitter.rs` into the lowering modules (a mechanical move, allowed early; see §6). |
| **1A.2–n** | Convert lowerings to `Node`, leaves first: literals, identifiers, operators, calls, then control flow and the list kernel. One expression family per slice. Each slice lowers `ts_writes` and the `Text`-bridge count. |
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
| **1B.2** | Move test-harness generation (`project/tests_emit/*`), `emit_composition_root` and the per-Worker emission out of `bynk-driver` and into `bynk-emit` (or a new crate; settling question Q3). The driver keeps orchestration, file I/O and process spawning. This removes `bynk-emit`'s dev-dependency on `bynk-driver`. |
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
| **1D.4** | Decide (settling question Q4) whether the wrangler-driven kinds move to REST, which would leave one credential and no matching on error text. If yes, convert them kind by kind, behind the 1D.3 trait. |
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

**Ratchet.** P0.5's `module_lines` becomes a ratchet when this phase starts. A file over the
threshold may not grow, and no file may newly cross it. Proposed threshold: 3,000 lines, falling
to 2,000 at exit (settling question Q5).

---

## 7. Phase 3 — rebalance the tests

Begins as each Phase 1 and Phase 2 seam lands. In practice it overlaps Phase 2: a module split
out carries its unit tests with it.

| Slice | Change |
|---|---|
| **3.1** | Unit tests at each new seam: a lowering returns a `TsExpr` to assert on, a `ComposePlan` decision, a `WorkerBindings` derivation, and a `ResourceKind`'s plan and apply against the 1D.3 fakes. |
| **3.2** | Re-measure coverage per crate with `cargo-llvm-cov`, as the [9 October review](../reviews/2026-10-09-unit-test-coverage-review.md) did, for own versus full coverage. Record the result as a new dated review. |
| **3.3** | Thin the corpus. A fixture whose only assertion is now made by a unit test is retired. Goldens remain for end-to-end shape, `tsc_verify` coverage and a curated set of behavioural run fixtures. The criteria are settling question Q6. |
| **3.4** | Per-crate floors on own-unit-test density (or own coverage, if 3.2 makes that cheap enough to gate), ratcheted. |

---

## 8. Exit criteria

| Measure | Today (v0.639.1) | Phase 1 exit (freeze lifts) | Track exit |
|---|---:|---:|---:|
| Gated growth probes | re-baselined (165 rises of `ts_writes`) | ratchets (P0.1) | ratchets |
| `ts_writes` (bynk-emit + driver `project/**`) | 1,502 | ≤ 40% of today | ≤ 20% |
| `ts_raw_sites`: `TsStmt::raw(` sites (emit + driver) | 99 | ≤ 10, each with a stated foreign-text reason | ≤ 5 |
| `ts_text_idents`: `Ident(format!(` sites (emit + driver) | 14 | 0 | 0 |
| `TsExpr` constructions in lowering | 0 | lowering returns `TsExpr` only; `Text` bridge deleted | — |
| Import-prediction functions | 17 | 0 | 0 |
| `"any"` fallbacks in lowering | 5 | 0 | 0 |
| Compose implementations | 2 | 1 (`ComposePlan`) | 1 |
| Places a binding kind is defined | ≥ 3 | 1 (`WorkerBindings`) | 1 |
| Deploy: apply consumes `Plan` | no | yes | yes |
| Deploy: Cloudflare client layers | 2, untyped | 1 trait, typed errors | 1 |
| `bynk-emit` dev-dependency on `bynk-driver` | yes | no | no |
| Crates denying wildcard arms | 2 (bynk-ts, bynk-model) | bynk-ts, bynk-model, bynk-emit | + bynk-driver, bynk |
| Back-half files ≥ 5,000 lines | 6 | — | 0 |
| Back-half files ≥ 2,000 lines | 20 | — | per Q5 |
| bynk-emit unit tests per 1k production lines | 7.1 | — | per 3.4's floor |

The `ts_writes`, `ts_raw_sites` and `ts_text_idents` targets are measured over bynk-emit and
bynk-driver's `project/**` *combined* (`codegen_family_files`). 1B.2 moves `tests_emit` from the driver into bynk-emit, so the move itself
is neither progress nor regression.

The back-half file rows count `src/` files in `bynk-model`, `bynk-emit`, `bynk-ts`, `bynk-driver`,
`bynk-strip` and `bynk`. On 1 October they read 5 and 10. Review §6.1's 18 and 50 count the whole
workspace.

The targets marked "≤ x%" are proposals for the settling review. The rest are structural yes/no.

---

## 9. Settling questions

- **Q1. Freeze scope.** Is §2's list right? In particular:
  - Should fixes to *documented-but-broken* features count as defects? (Proposed: yes.)
  - Should `bynk-std` additions that need no emitter change be allowed? (Proposed: no, for
    simplicity.)
- **Q2. Golden churn in 1A.** Is "layout-only, proven by `tsc_verify` plus run fixtures" enough? Or
  should 1A first make the printer reproduce today's hand formatting, so that goldens do not move
  at all? (Proposed: accept layout-only diffs. Reproducing hand formatting would pin the debt in
  place.)
- **Q3. Where test generation lives.** A `bynk-emit::testgen` module, or a new `bynk-testgen`
  crate? (Proposed: a module first. Promote it to a crate only if build times argue for it.)
- **Q4. Wrangler or REST for resource operations.** Moving everything to REST gives one
  credential, typed errors and no matching on error text. But it re-implements what wrangler does,
  and diverges from the documented `wrangler login` flow. (Open. 1D.3's trait makes the decision
  reversible.)
- **Q5. The module-size threshold.** 3,000 lines falling to 2,000, or a different measure, such as
  items per module?
- **Q6. Corpus-thinning criteria for 3.3.** Who decides that a fixture's assertion is "made by a
  unit test", and how is it recorded? (Proposed: the retiring PR names the unit test per fixture.)
- **Q7. ADR posture.** Which of §10's decisions need an ADR before slicing starts?

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
