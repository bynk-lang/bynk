# Retired feature tracks — the closing summaries

The historical record of completed [feature tracks](../tracks/README.md). A
track's retirement PR removes its doc from `design/tracks/` (the decisions live
on in the [ADRs](../decisions/README.md) and the spec-in-place), appends its
closing summary here — what shipped, which ADRs carry its decisions, and the
named follow-ons — and closes the track's spine issue. Newest first is not
imposed; entries keep the order they were retired in.

- **`compiler-architecture.md`** — phases 0–2 of
  [`../bynk-compiler-trajectory.md`](../bynk-compiler-trajectory.md): the compiler testable at crate
  granularity (phase 0, Seams), the named small defects closed with the registries single-sourced
  (phase 1, Paydown), and `lower_expr` returning what it produced instead of appending it to a
  caller-supplied sink (phase 2, Typed lowering) — none of it moving the language surface. Settled
  1 August 2026 (settling PR [#997](https://github.com/accuser/bynk/pull/997), v0.246.1), then sliced.
  Ten Tier A slices plus the decision slice shipped (v0.246.2–v0.247.0): **T0.0** — `cargo xtask
  greenfield-status`, the probe harness itself
  ([#999](https://github.com/accuser/bynk/issues/999)/[#1000](https://github.com/accuser/bynk/pull/1000),
  v0.246.2); **T-D1** — `bynkc`'s published API narrowed to its ~30 item re-exports, deleting the
  fourteen whole-module re-exports
  ([#1004](https://github.com/accuser/bynk/issues/1004)/[#1005](https://github.com/accuser/bynk/pull/1005),
  v0.246.4, [ADR 0312](../decisions/0312-narrow-bynkc-public-api.md)); **T0.2′** —
  `expected_diagnostics.txt` adopted for the `Roots::Split` fixtures ADR 0198 named as unobservable
  ([#1007](https://github.com/accuser/bynk/issues/1007)/[#1008](https://github.com/accuser/bynk/pull/1008));
  **T0.3** — `[workspace.lints]` added with `wildcard_enum_match_arm` at `warn`
  ([#1010](https://github.com/accuser/bynk/issues/1010)/[#1011](https://github.com/accuser/bynk/pull/1011),
  v0.246.6); **T0.4′** — `tree-sitter-bynk`'s conformance suite widened from a fixed case list to
  totality (PR [#1015](https://github.com/accuser/bynk/pull/1015)); **T0.7** — filesystem reads below
  the driver, partial
  ([#1006](https://github.com/accuser/bynk/issues/1006)/[#1012](https://github.com/accuser/bynk/pull/1012):
  `bynk-fmt` cleared, `bynk-emit`/`bynk-ide` not); **T1.6′** — Query's `method_not_found` generated
  from `QUERY_METHODS`, the drift test made bidirectional
  ([#1009](https://github.com/accuser/bynk/issues/1009)/[#1014](https://github.com/accuser/bynk/pull/1014),
  v0.246.8); **T1.7′** — the wildcard-arm/shadowing residue verified already closed
  ([#1020](https://github.com/accuser/bynk/issues/1020)/[#1022](https://github.com/accuser/bynk/pull/1022));
  **T1.8** — `NonEmpty` folded into `MinLength(1)`, partial
  ([#1021](https://github.com/accuser/bynk/issues/1021)/[#1027](https://github.com/accuser/bynk/pull/1027),
  v0.247.0: `Positive`/`NonNegative` not folded). Tier B — the typed hoist, R6.2–R6.4 — shipped as
  three slices (v0.247.1–v0.247.4): **T2.1** — `lower_expr` returns `Lowered { pre, expr }` instead of
  taking a `stmts: &mut Vec<String>` sink, closing the dropped- and spliced-statement bugs across ~90
  functions
  ([#1017](https://github.com/accuser/bynk/issues/1017)/[#1029](https://github.com/accuser/bynk/pull/1029));
  **T2.2** — `maybe_async_iife`'s `contains("await ")` text scan replaced by
  `LowerCtx::emitted_await`, a flag set at lowering time
  ([#1018](https://github.com/accuser/bynk/issues/1018)/[#1042](https://github.com/accuser/bynk/pull/1042));
  **T2.3** — the `?`-under-short-circuit-operand escape closed by the same flag-on-`LowerCtx` shape
  (`emitted_early_return`), routing a flagged right operand through a hoisted `if` statement instead of
  an arrow-IIFE
  ([#1019](https://github.com/accuser/bynk/issues/1019)/[#1044](https://github.com/accuser/bynk/pull/1044),
  v0.247.4). `hoist_sinks` (`rg 'stmts: &mut Vec<String>' bynk-emit/`) reads **0**. Decisions in ADRs
  [0309](../decisions/0309-refactor-acceptance-gate-per-tier.md) (the refactor acceptance gate,
  per-tier), [0310](../decisions/0310-the-emit-abi-is-published-the-codegen-is-not.md) (the emit ABI
  is published, the codegen is not),
  [0311](../decisions/0311-the-lowering-substrate.md) (the lowering substrate — recorded as
  deliberate, amended by `Lowered`, with phase 3's triggers named), and
  [0312](../decisions/0312-narrow-bynkc-public-api.md) (`bynkc`'s published API is its item
  re-exports, not its module structure). Surface lives in `bynk-emit/src/emitter.rs`/`lower.rs`,
  `bynkc/src/lib.rs`, `xtask/src/greenfield_status.rs`, and `tree-sitter-bynk/tests/conformance.rs`.

  **The phase-3 trigger fired, against [ADR 0311](../decisions/0311-the-lowering-substrate.md)'s own
  wording — not the paraphrase this track's §3.4 carried.** ADR 0311 D3's third trigger starts its
  two-release clock "after a crate-local test seam exists," not after Tier A completes; the seam
  (`CompileOptions.sources`, Wave 3, [#954](https://github.com/accuser/bynk/pull/954)) landed at
  v0.238.0, nine minor versions before this track's own probe harness began tracking it. `bynk-emit`'s
  test-line density has read flat at 8.3–8.5% for the whole precisely-measured window (v0.246.1
  onward), after an initial post-seam rise from the July review's 6.5% baseline — the plateau ADR 0311
  named as the evidence a substrate replacement would need. More directly, D3's first trigger — a
  defect class recurring after being patched once at a different site — is recorded in the ADR itself
  as having already happened once, at `maybe_async_iife`, "not recognised as a signal." Tier B then
  reproduced the identical shape a further time: T2.3's `emitted_early_return` is built as a copy of
  T2.2's `emitted_await` mechanism, because the same defect class — a control-flow property inferred
  by local, textual reasoning instead of tracked structurally through lowering — surfaced at a third
  site after the ADR naming the pattern was already on file. This closes into a new phase-3 track
  ([#1046](https://github.com/accuser/bynk/issues/1046)) rather than the state-migrations track §3.6
  anticipated running next; the two are independent and neither blocks the other.

  **Deferred follow-ons, named not silently assumed away:** three Tier A slices shipped narrower than
  their row named, and nothing tracked the remainder until now — **T0.7's remaining six filesystem
  reads below the driver** (`bynk-emit`=4, `bynk-ide`=2; only `bynk-fmt` was cleared, in
  [#1012](https://github.com/accuser/bynk/pull/1012)), filed as
  [#1047](https://github.com/accuser/bynk/issues/1047); **T-D1/R10.4's remaining whole-module
  re-exports** — `bynk-syntax` (7 modules), `bynk-driver` (2), and `bynk_fmt as fmt` (the whole crate)
  are still re-exported whole from `bynkc/src/lib.rs`, the same defect T-D1 closed for
  `bynk-check`/`bynk-emit` — filed as [#1048](https://github.com/accuser/bynk/issues/1048); **T1.8's
  `Positive`/`NonNegative` predicate fold**, blocked on a real ambiguity (`Positive`/`NonNegative` on
  `Float` has no clean exclusive-vs-inclusive-bound answer, #1021's Decision A) — filed as
  [#1049](https://github.com/accuser/bynk/issues/1049). A fourth item, T0.3's per-crate `deny`
  rollout (`[workspace.lints]` currently `warn`, zero crates opted in via `[lints] workspace = true`),
  was **deliberately** left incremental by its own row ("`deny` per crate as each is cleared") and is
  not residue in the same sense — no issue filed; each crate's `deny` graduation rides its own future
  PR.
- **`testing-the-boundary.md`** — the rung the retired `testing.md` subject
  ladder never had: the **boundary**. Bynk's pitch rests on the edge — types enforced,
  identity sealed, the author writing neither check — yet *no Bynk test could observe
  any of it*: across the fixtures, the set of tests that both drove a `from http`
  service and asserted a boundary claim was empty, and `scheduled`/`queue` had never
  been executed by anyone, including the compiler. The track taught the existing tier
  dial ([ADR 0153](../decisions/0153-tier-is-a-dial-on-the-case-header.md)) the entry
  it was never taught — **no new axis, no new harness**. All four planned slices
  shipped: **0** (v0.181, [ADR 0203](../decisions/0203-test-body-service-calls-resolved.md),
  #662) — the checker resolves the addressed handler (closed the #654 crash); **A**
  (v0.185, [ADR 0205](../decisions/0205-unit-tier-service-address.md), #664) — the
  unit-tier surface: address `http`/`cron`/`queue` from a `case` with
  `by <Actor>(<identity>)`, giving `scheduled`/`queue` their first-ever execution
  coverage; **B** (v0.187, [ADR 0207](../decisions/0207-system-tier-http-boundary.md),
  #667/#697) — the system-tier boundary: drive an http route over a real `worker.fetch`
  with a framework-signed credential the real auth seam verifies, `system_needs_wire`
  relaxed to a serialisation edge; **C** (v0.189,
  [ADR 0210](../decisions/0210-system-tier-wire-rejection.md), #702/#704) — the
  rejection paths: `Wire(<String>)` hands the router raw, pre-validation input so a
  case observes the boundary *reject* it (`Rejected`) or *handle* it (`Handled`),
  decoded on shape not status. Along the way the track's own thesis-in-miniature
  surfaced and closed a real defect: boundary-rejection `400`s shipped without
  `nosniff` ([#659](https://github.com/accuser/bynk/issues/659), v0.188.1,
  [ADR 0209](../decisions/0209-boundary-rejection-security-headers.md)) — *the
  router's behaviour is exactly what no Bynk test could observe*. Surface lives in
  `bynk-emit/src/project/tests_emit.rs` (the test emitter), `bynk-check/src/checker/calls.rs`
  (address resolution), and the `responseToHttpResult`/`responseToHttpOutcome` runtime
  decoders. **Deferred follow-ons** (none blocking the theme, all from ADR 0210):
  rejection-*kind* discrimination ([#705](https://github.com/accuser/bynk/issues/705) —
  `is` tests one level), the `401` path
  ([#706](https://github.com/accuser/bynk/issues/706) — needs a credential override),
  the `405` fall-through ([#707](https://github.com/accuser/bynk/issues/707) — needs
  wrong-method addressing), and mixed typed+`Wire` arguments
  ([#708](https://github.com/accuser/bynk/issues/708)).
- **`editor-currency.md`** — a tooling track closing the drift between what the
  Bynk language *is* and what the editor surface (`bynk-lsp` + `vscode-bynk`)
  shows: hover, completion, scaffolds, menus/keybindings, and codelens brought
  back in step with the language, and held there by a **mechanical floor** so the
  next language slice cannot silently re-open the gap. All six slices shipped
  (v0.121–v0.127): the guardrail — a keyword coverage test + a scaffold-compiles
  test (slice 0, v0.121) → parameter/local hover (1, v0.122) → hover depth for
  declarations (2, v0.123) → completion depth (3, v0.124) → scaffold refresh
  (4, v0.125) → the VS Code UI surface, menus/keybindings/editor-config (5,
  v0.126) → codelens depth, the per-case test-run filter + the capability
  provider lens (6, v0.127) — plus two named fast-follows: `match`-arm pattern
  completion (v0.128, the deferred half of slice 3) and the refinement-family
  codelens (v0.129, closing [#259](https://github.com/accuser/bynk/issues/259) —
  the parked `refines` half of slice 6). Decisions in ADRs
  [0156](../decisions/0156-editor-surface-tracks-language.md) (the editor surface
  tracks the language, with a mechanical floor over hover and completion) and
  [0157](../decisions/0157-scaffolds-cannot-drift.md) (editor scaffolds cannot
  drift — each catalogue compiles in CI, independently). The guardrail lives in
  `bynk-lsp/tests/editor_coverage.rs` + `scaffolds_compile.rs`; the surface is
  `bynk-lsp` (hover/completion/signature-help/codelens) and `vscode-bynk` (the
  manifest). Tooling-only — no language, grammar, or emitter-output change across
  the whole track. **Deferred follow-ons** (none blocking the theme): the
  Marketplace publish ([#258](https://github.com/accuser/bynk/issues/258)) and
  editor-agnostic docs ([#257](https://github.com/accuser/bynk/issues/257)).
- **`testing.md`** — one predicate surface: a far-reaching rethink of how Bynk expresses
  tests, unifying examples, properties, contracts, invariants, and interaction checks as
  facets of the **invariant predicate** the language already has — along a ladder of
  subjects (value → domain → call → snapshot → step → history), sourced by
  supply-or-generation, checked at one of three checkpoints (commit boundary / dev call
  site / test runner), and run at one of three tiers (`unit`/`integration`/`system`). It
  sharpened the testing philosophy and reference and extended the agent-invariant model's
  thesis — *"invariants are the contract half of validation; tests are the behaviour
  half."* All slices shipped (v0.112–v0.119): (1a) `expect` + `suite`/`case` with
  structural failure reporting (v0.112) → (1b) structural test-ness + flat `[paths]`
  (v0.113) → (2) `property`/`for all` and `Val[T]` replacing `Mock[T]` (v0.114) →
  (3) function contracts `requires`/`ensures` (v0.115) → (4) step invariants `transition`
  (v0.116) → (5) the observation surface `expect Cap.op called …` + `trace` (v0.117) →
  (6) the tier dial `as unit | integration | system` + per-seam `provides`, retiring
  `mocks`/`suite integration`/`wires` (v0.118) → (7) history properties
  `for all run: History[Agent]`, the visionary tail (v0.119). Decisions in ADRs
  [0144](../decisions/0144-one-predicate-surface.md) (one predicate surface, landed up
  front), [0145](../decisions/0145-expect-replaces-assert.md) (`expect` replaces
  `assert`), [0146](../decisions/0146-suite-case-vocabulary.md) (`suite`/`case`
  vocabulary), [0147](../decisions/0147-structural-test-ness-and-flat-paths.md)
  (structural test-ness & flat paths),
  [0148](../decisions/0148-val-replaces-mock.md) (`Val[T]` replaces `Mock[T]`),
  [0149](../decisions/0149-generation-is-valid-inhabitants.md) (generation is valid
  inhabitants), [0150](../decisions/0150-contracts-are-invariants-for-functions.md)
  (contracts are invariants for functions),
  [0151](../decisions/0151-the-invariant-subject-widens-to-the-step.md) (the invariant
  subject widens to the step),
  [0152](../decisions/0152-observation-is-auto-recorded-at-the-capability-seam.md)
  (observation auto-recorded at the seam),
  [0153](../decisions/0153-tier-is-a-dial-on-the-case-header.md) (tier is a dial on the
  case header), [0154](../decisions/0154-test-doubles-are-provides.md) (test doubles are
  `provides`), and [0155](../decisions/0155-history-properties-are-runner-only.md)
  (history properties are runner-only); spec-in-place in
  `site/src/content/docs/book/spec/syntactic-grammar.md` + `static-semantics.md` and
  `site/src/content/docs/book/reference/testing.md` + `agent-invariants.md`, with
  `guides/testing/philosophy.md` the keystone rewrite around the spine. **Deferred
  follow-ons** (none blocking the theme): multi-agent protocol properties (the history
  rung is single-agent only — ADR 0155); the universal-emission guarantee that still has
  no home (design DECISION U); a declaration-positional enum `Ord` for ordered-status
  transitions (DECISION O); and whether `example` earns its own keyword over a
  pinned-subject single-case `for all`.
- **`in-browser.md`** — the Browser platform, the JS emit path, the wasm toolchain, and
  the in-browser REPL/playground. Realised design notes §18 (Tier-3 platform bindings)
  and §19 (additional backends; the "a REPL is ambitious and probably v2 or v3" aside) —
  turning the zero-install playground the design notes always pointed at into a shipped
  on-ramp. All slices shipped (v0.108.0–.5): the strip-only emission invariant (0), the
  first-class JS artefact `--emit js` (1), the `--platform browser` binding (2), the
  wasm toolchain `bynk_compile` (3), the REPL/playground itself (4), and slice-5 polish —
  an examples gallery, web-tree-sitter highlighting, a snippet-share service **written
  in Bynk**, and live on-type diagnostics. Decisions in ADRs
  [0136](../decisions/0136-strip-only-emission-invariant.md) (strip-only emitter),
  [0137](../decisions/0137-first-class-js-artefact.md) (JS artefact),
  [0138](../decisions/0138-browser-platform.md) (Browser platform),
  [0139](../decisions/0139-wasm-toolchain.md) (wasm toolchain), and
  [0140](../decisions/0140-repl-execution-and-sandbox.md) (REPL execution & sandbox); the
  playground app lives in `playground/` (outside the Rust workspace). **Deferred
  follow-ons** (none blocking the theme): Cloudflare Pages deployment (two projects +
  DNS), a share-id persistence upgrade beyond the hash form, and LSP-in-browser
  hover/completion. Bynk's `from http` gained no CORS in the process — a noted candidate
  future language feature (same-origin routing sidesteps it for the playground).
- **`websocket.md`** — real-time Bynk: the `Stream[T]` value-over-time primitive, a
  streaming-HTTP (SSE-shaped) response terminal consuming it, and the `from WebSocket`
  protocol with held `Connection[F]` resources transferred from a service to an agent.
  Realised design notes §7 (the WebSocket protocol) and §20 Example 2 (the chat-room),
  and sharpened `bynk-type-system.md` §2.9 (`Held[T]`/`Connection[F]` linearity). All
  slices shipped (v0.100–v0.107): `Stream[T]` (0), streaming HTTP (1), held-resource
  linearity (2), the `from WebSocket` bundle (3a), Workers edge-auth + DO-hosted on-open
  (3b-i), hibernation (3b-ii), inbound `on message`/`on close` (3b-iii), and broadcast +
  the §20 chat-room end-to-end (4). Decisions in ADRs
  [0128](../decisions/0128-stream-value-over-time-primitive.md) (`Stream[T]` primitive),
  [0129](../decisions/0129-streaming-http-response.md) (streaming-HTTP response),
  [0130](../decisions/0130-held-resource-linearity.md) (held-resource linearity),
  [0131](../decisions/0131-from-websocket-protocol-bundle.md) (`from WebSocket` bundle),
  [0132](../decisions/0132-from-websocket-protocol-workers.md) (Workers edge-auth +
  on-open), [0133](../decisions/0133-from-websocket-hibernation.md) (hibernation),
  [0134](../decisions/0134-from-websocket-inbound.md) (inbound frames), and
  [0135](../decisions/0135-ws-broadcast-closure.md) (broadcast + closure); spec-in-place
  in `site/src/content/docs/book/spec/syntactic-grammar.md` + `static-semantics.md` and
  `site/src/content/docs/book/reference/websocket.md`. **Deferred follow-ons** (none blocking the theme):
  the `.values` accessor, lambda parameter-type inference, a non-Cloudflare `Connection`
  binding, and a streaming `Ai`/`Queue`-out consumer.
- **`storage.md`** — the agent-local storage-kind catalogue of design notes §10:
  `store` fields replacing the `state { }` record, the five kinds
  (`Cell`/`Map`/`Set`/`Cache`/`Log`; `Queue` ruled out as a delivery concern), the
  `:=`/kind-op write forms, access-pattern annotations, the parity cutover, and
  load-time rehydration validation. All slices shipped (v0.82–v0.97): `Cell` +
  handler-atomic commit (0/1), `Map` (2), `Set` (3), the annotation surface (3a),
  the `Duration` primitive (3b), `Cache` (3c), `Log` (4), the **parity cutover**
  removing `state { }`/`commit`/`self.state` (1p, v0.96), and the **rehydration
  validation gate** (6r, v0.97). Decisions in ADRs
  [0108](../decisions/0108-state-record-to-store-fields.md) (`store` replaces
  `state { }`), [0109](../decisions/0109-handler-atomic-commit.md) (handler-atomic
  commit), [0110](../decisions/0110-storage-map-vs-value-map.md) (`Map`
  storage-vs-value by receiver provenance),
  [0111](../decisions/0111-storage-annotation-surface.md) (annotation surface),
  [0112](../decisions/0112-duration-primitive.md) (`Duration`),
  [0113](../decisions/0113-cache-ttl-eviction.md) (`Cache` TTL eviction),
  [0121](../decisions/0121-log-append-and-retention.md) (`Log` append/retention),
  [0122](../decisions/0122-queue-is-a-delivery-concern.md) (`Queue` is a delivery
  concern, not a storage kind),
  [0123](../decisions/0123-state-block-cutover-and-codemod.md) (the parity cutover),
  and [0124](../decisions/0124-rehydration-validation-and-migration.md) (rehydration
  validation). Spec-in-place in `site/src/content/docs/book/spec/syntactic-grammar.md` +
  `static-semantics.md` and `site/src/content/docs/book/reference/agents.md` + `grammar.md`.
  **Deferred follow-ons** (none blocking the theme): a versioned-schema migration
  capability, per-field default-on-read, a soft recovery handler, whole-collection
  invariant quantifiers (ADR 0123 D4), per-entry DO storage keys, and refined
  non-textual-key rehydration validation (ADR 0124 D5).
- **`query-algebra.md`** — the read/transform combinator vocabulary of design
  notes §11 (lazy `Query[T]` on storage, eager on in-memory collections; builders
  + terminals; `@indexed` secondary indexes with build-time hygiene; joins &
  grouping). All core slices shipped (v0.88–v0.94): the eager `List` vocabulary
  (slice 1), the `Instant` primitive (1b), the `bynk.list`→methods deprecation
  (1c), the lazy `Query` over storage `Map` (2), `@indexed` with routing + hygiene
  warnings (3), and joins & grouping in the **combiner form** (4). Decisions in ADRs
  [0114](../decisions/0114-instant-primitive.md) (`Instant`),
  [0115](../decisions/0115-query-model-lazy-eager-dispatch.md) (`Query[T]` model +
  dispatch), [0116](../decisions/0116-query-vocabulary-and-ordering.md) (vocabulary
  + `Ordering`), [0117](../decisions/0117-non-failing-warning-channel.md) (the
  non-failing warning channel — built here as a prerequisite),
  [0118](../decisions/0118-indexed-indexing-model.md) (`@indexed`),
  [0119](../decisions/0119-durable-object-query-lowering.md) (DO lowering), and
  [0120](../decisions/0120-join-group-combiner-form.md) (the combiner form, no pair
  type); spec-in-place in `site/src/content/docs/book/spec/static-semantics.md` (the query-vocabulary
  section). **Deferred follow-ons** (none blocking the theme): in-memory effectful
  iteration as a uniform method surface (`traverse`/`traverseAll`/`parTraverse`/
  `parTraverseAll` — the original slice 5, tangential to read/transform querying;
  needs its own settling vs the existing `bynk.list.traverse`); the cross-shape
  `Map × Log` join + `Log` time-window builders (land with the storage `Log` slice);
  `@indexed`'s `bynk.index.ambiguous` note + add/remove auto-fixes (await
  compound-predicate routing); **labelled call arguments** (would realise the join
  combiners' `left:`/`right:`/`into:` named surface — v1 is positional); a general
  n-ary **tuple**; and per-entry DO storage keys (turn the index/query CPU wins into
  I/O wins).
- **`debugging.md`** — source-mapped step debugging for Bynk. **Phase 1** (the
  pragmatic base: breakpoints, stepping, and the call stack on `.bynk` source under
  the Node test runner and `workerd`/`wrangler dev`) shipped over v0.67–v0.72 (slices
  0–4), plus **Phase 2's on-ramp** (slice 5, v0.73: value descriptions via js-debug's
  in-debuggee generator). Reuses VS Code's JavaScript debugger via a thin
  `DebugConfigurationProvider` — no bespoke Debug Adapter. Decisions in ADRs
  [0103](../decisions/0103-source-map-contract.md) (source-map contract) and
  [0104](../decisions/0104-debug-launch-model.md) (debug-launch model); guide at
  `site/src/content/docs/book/guides/editor-and-tooling/debugging.md`. Phase 2's remainder was carried
  by `semantic-debugging.md` below.
- **`semantic-debugging.md`** — making the debugger *speak Bynk*: an editor-side
  `DebugAdapterTracker` that rewrites js-debug's `variables`/`scopes`/`stackTrace`
  responses into Bynk's vocabulary (runtime-agnostic, so it reaches `workerd`). Slices
  0–4 (v0.74–v0.77) shipped: the interposition model, values on both runtimes,
  capabilities/state as frame groups, the call stack named by Bynk operation (with the
  emitter `<file>.bynkdbg.json` sidecar), and lowered-temp suppression. Decision in ADR
  [0105](../decisions/0105-semantic-debug-interposition.md). The one named follow-on —
  surfacing the `by` actor in the frame — is parked in
  [issue #286](https://github.com/accuser/bynk/issues/286).
- **`crate-decomposition.md`** — a tooling track: `bynkc` decomposed from a
  monolith into a layered library set
  (`bynk-syntax`/`-render`/`-fmt`/`-check`/`-emit`/`-ide`), the human CLI moving
  up into the driver. All slices shipped (v0.60–v0.66); decisions in ADRs
  [0099](../decisions/0099-crate-layering-dependency-direction.md)–[0102](../decisions/0102-foundation-types-boundary.md)
  (+ the 0084 amendment).
- **`actors.md`** — actor declarations as boundary contracts (the `actor`
  declaration, the `by` clause, authentication schemes, identity). Q1–Q7 shipped
  (v0.45–v0.54); decisions in ADRs
  [0080](../decisions/0080-actor-schemes-closed-nominal.md)–0082, 0085,
  0088–[0092](../decisions/0092-cross-context-caller-value.md). The inaugural
  feature track. Q8 (replay/ordering) deferred to a future Events track —
  [issue #260](https://github.com/accuser/bynk/issues/260).
- **`lsp.md`** — the editor-experience connective plan (completion overhaul,
  navigation round-out, editor polish). Slices 0–7 + 9 shipped (v0.24–);
  decisions in ADRs
  [0093](../decisions/0093-completion-surface-contract.md)–[0095](../decisions/0095-unit-source-map.md),
  with the feature spec in [`../bynk-lsp-spec.md`](../bynk-lsp-spec.md). Remaining
  work tracked in issues
  [#257](https://github.com/accuser/bynk/issues/257) (editor-agnostic docs) and
  [#258](https://github.com/accuser/bynk/issues/258) (marketplace publishing).
  ([#259](https://github.com/accuser/bynk/issues/259), refinement-families nav,
  shipped v0.129 under the retired `editor-currency.md` track.)
- **`lsp-foundations.md`** — the foundation *under* the shipped LSP surface. An
  external review found the feature surface "unusually feature-complete" and then
  four foundational gaps that shared one shape: every gap was in the
  transport/lifecycle layer, and every test of that layer asserted on static
  shape rather than behaviour over time. The LSP analysed a different project than
  `bynkc`; cached rounds had no freshness gate; workspace folders were advertised
  but unimplemented; there was no startup analysis or dynamic watcher
  registration. Seven slices closed them (v0.175–v0.184): **0** — file identity, a
  project-relative `identity_path` beside the tree-relative `source_path` unit
  validation needs ([ADR 0198](../decisions/0198-file-identity-is-not-the-unit-validation-path.md));
  **A** — one project model, `bynk-ide` reading the manifest's `[paths]`
  `include`/`exclude` exactly as `bynkc` does, so the server analyses the *same*
  files ([ADR 0201](../decisions/0201-the-lsp-analyses-the-compilers-project-model.md));
  **B** — the freshness contract, an index-backed request refreshing to the buffer
  the client holds, never answering stale ([ADR 0202](../decisions/0202-the-freshness-contract.md));
  **C** — the `[lib]` seam, the server moved to `src/lib.rs` so integration tests
  name the crate and the `#[path]` hack retired (no ADR — a refactor); **D** —
  per-workspace state, a project-root-keyed map routing by the file's nearest
  `bynk.toml`, `did_change_workspace_folders`, the multi-root capability made true
  ([ADR 0204](../decisions/0204-per-workspace-project-state.md)); **E** — startup
  analysis + server-registered watchers, a `bynk.toml` tree-walk warming every
  project on activation and the server registering `didChangeWatchedFiles` itself
  so any client is notified (no ADR); **F** — one diagnostics scheduler, a single
  generation-based debounce over both modes at the configured delay (no ADR).
  Q1–Q6 all settled; the recurring lesson was to trace how each handler *uses* the
  analysis, not just how it reads it (rename *writes* many files → needs a
  whole-buffer freshness gate). The decisions live on in ADRs
  [0198](../decisions/0198-file-identity-is-not-the-unit-validation-path.md)/[0201](../decisions/0201-the-lsp-analyses-the-compilers-project-model.md)/[0202](../decisions/0202-the-freshness-contract.md)/[0204](../decisions/0204-per-workspace-project-state.md)
  and the spec-in-place ([`../bynk-lsp-spec.md`](../bynk-lsp-spec.md), consolidated
  in slice G). The **first track to run the [ADR 0167](../decisions/0167-feature-tracks-run-github-native.md)
  GitHub-native flow from the start** — spine issue first, doc via a settling
  draft PR. **Deferred follow-ons** (none blocking the theme): a per-URI root cache
  (routing re-walks the FS per request — a static→stateful change, its own perf
  increment), and the capability depth the spec's §8 now lists — local-binding
  rename, match-arm navigation, the consumed-context navigation half, auto-import,
  a test-run codelens.
- **`increment-allocation.md`** — a process track that removed the one structural
  reason parallel increments conflicted: the repo version and the ADR number are
  two strictly-increasing counters, and both were transcribed into shared files
  *while a PR was authored* (`scripts/bump-version.sh` into ~15 files; the ADR file
  + its index row), so two increments developed in parallel collided on every
  version-bearing file and the index table — and the loser silently shipped a
  number another had already taken. The fix: a feature PR now declares only
  **intent** in one `design/pending/<slug>.md` (bump level, changelog blurb, ADR
  prose — **no numbers**), and a per-merge automation assigns the counters on
  `main`, in merge order, where they cannot collide. All four slices shipped:
  the pending-increment format + a `cargo xtask check-pending` validator, in a new
  unpublished `xtask` workspace member (slice 0, [#688](https://github.com/accuser/bynk/pull/688))
  → the `cargo xtask stamp` command — read the pending files, assign each its next
  version + ADR number, run `bump-version.sh`, prepend the changelog row(s), write
  the numbered ADR(s) + index row(s), and delete the consumed file (slice 1,
  [#690](https://github.com/accuser/bynk/pull/690)) → the per-merge GitHub Actions
  workflow that runs the stamp on merge and pushes the result (slice 2,
  [#692](https://github.com/accuser/bynk/pull/692), v0.186) → `llms-full.txt` moved
  from a committed, drift-guarded copy to a build artifact regenerated by a
  `prebuild` npm hook (slice 3, the deferrable surface-shrink,
  [#694](https://github.com/accuser/bynk/pull/694)). The load-bearing decision lives
  in ADR [0206](../decisions/0206-allocation-on-main.md) (allocation happens on
  `main` by automation, in merge order — **per-merge, not a batched release PR**,
  because the ADR number must be assigned at every merge regardless; and
  **delete-what-you-consume** idempotency, which entangles version and ADR
  assignment into one atomic pass — the finding that folded the original slices 1
  and 2 into one and split the workflow out as its own). Two slices carried no ADR
  (infra-only). It lives on in the `xtask` crate (`stamp` / `check-pending`),
  `.github/workflows/stamp.yml`, `design/pending/` and its README, and ADR 0206.
  **Named follow-ons / caveats:** the **first live stamp run had not yet happened
  at retirement** — the workflow fires on the next increment that carries a
  `design/pending/` file, and that merge is the real end-to-end test; the
  surface-shrink stopped at `llms-full` (the 12 crate READMEs are published to
  crates.io and the 7 Book banners were not worth an Astro build-time injection, so
  both stay committed); and the mechanism depends on `main` being **unprotected**
  (the default `GITHUB_TOKEN` pushes directly and triggers no CI) — if `main` is
  ever branch-protected, the stamp must move to a GitHub App with a push-bypass or
  a stamp PR, as ADR 0206 records.
- **`deploy.md`** — the `bynk deploy` verb: provisioning + remote deploy, the
  capstone of the driver arc `doctor → new → dev → deploy`, realising the
  tooling roadmap §5.1 and the deferral `bynk dev` (ADR 0096 D4) named by
  name: "real, provisioned remote support is `deploy`'s defining problem, the
  next slice." The track's one genuinely new idea — the **provisioning-state
  model** (`bynk.deploy.lock`, the deploy-time analogue of `bynk.lock`: real
  Cloudflare resource ids live in persistent driver state, injected into
  regenerated config just before Wrangler runs, never sourced from it) — and
  its one genuinely new responsibility: this is the **first driver command
  with irreversible, outward-facing side effects**, the reason it was a track
  and not a fourth additive verb. All six slices shipped: **0** — the
  provisioning-state model + KV-only single-context MVP (v0.154,
  [ADR 0179](../decisions/0179-deploy-provisioning-state.md)/[0180](../decisions/0180-deploy-orchestration-idempotency.md),
  [#583](https://github.com/accuser/bynk/issues/583)) → **1** — DO migrations
  + queue provisioning, queues reconciling by create-every-run-and-treat-
  already-exists-as-success (v0.171,
  [ADR 0194](../decisions/0194-deploy-queues-and-delegated-do-migrations.md),
  [#600](https://github.com/accuser/bynk/issues/600)) → **2** — multi-context
  topology + Service-Binding deploy ordering, confirming empirically that
  Cloudflare resolves bindings at upload (a hard barrier, not a soft nicety)
  (v0.170, [ADR 0193](../decisions/0193-multi-context-deploy-ordering.md),
  [#601](https://github.com/accuser/bynk/issues/601)) → **3** — secrets at
  deploy time, the declared/read/supplied floor-not-census contract, values
  moved to `wrangler secret put` on stdin and never persisted (v0.172,
  [ADR 0195](../decisions/0195-secrets-at-deploy.md),
  [#602](https://github.com/accuser/bynk/issues/602); follow-up
  [#632](https://github.com/accuser/bynk/issues/632) on computed
  `Secrets.get` names, [ADR 0196](../decisions/0196-secret-reads-and-computed-names.md))
  → **4** — environments, `--env` threaded through the ledger and a
  driver-synthesised `[env.<name>]` config section (confirmed against
  Cloudflare's own docs that bindings are non-inheritable into a named
  environment, so the emitter — which never sees a deploy-time environment
  name — could not do this itself), queue/Service-Binding names qualified to
  avoid cross-environment collision, extended in the same PR to
  `bynk dev -- --remote` after review caught it reading only the default
  ledger section (v0.220.1,
  [ADR 0254](../decisions/0254-deploy-environments.md),
  [#835](https://github.com/accuser/bynk/issues/835)) → **5** — reconciliation
  maturity: per-resource-kind orphan reporting (a pure offline ledger-vs-source
  diff, so `--dry-run` never authenticates), KV drift detection once per
  deploy run (closing the one asymmetry where queues already self-healed but a
  deleted-out-of-band KV namespace did not), and `--prune` scoped to KV
  namespaces and queues alone — never a Worker, whose blast radius (routes,
  custom domains, cron triggers) is categorically larger — with idempotent
  deletion confirmed empirically against a real account rather than assumed
  (v0.220.2, [ADR 0255](../decisions/0255-deploy-reconciliation.md),
  [#839](https://github.com/accuser/bynk/issues/839)). Spec-in-place in
  `site/src/content/docs/book/guides/projects-build-and-deployment/deploy-to-cloudflare.md`
  and `run-locally.md`; surface lives in `bynk/src/deploy.rs`. **Deferred
  follow-ons** (none blocking the theme): release semantics — rollback,
  versioned deploys, traffic splitting — stayed an explicit non-goal
  throughout (§2), noted for a future track; the pre-flight's account
  selection stays `wrangler`-deferred and account-blind (a user pointed at
  the wrong Cloudflare account for a given environment still gets a
  pre-flight pass); pruning an orphaned Worker (`wrangler delete`) and the
  race window between `--prune`'s report and its delete call are both named,
  unclosed gaps in [ADR 0255](../decisions/0255-deploy-reconciliation.md).
  **One unresolved, load-bearing risk, not just a nice-to-have:** the
  packaging track (still an uncommitted local draft with no spine issue of
  its own) plans to re-address contexts as `org.package.context`, and
  `deploy`'s Worker names and ledger keys assume today's flat identity —
  whoever picks up packaging must sequence its naming cutover against
  `deploy`'s provisioned state, or a rename orphans already-live resources
  with no automatic recovery.
- **`message-bundles.md`** — the sibling `locale-capability.md` named but left
  unfiled at settling time: turned [ADR 0256](../decisions/0256-locale-capability-slice-1.md)'s
  shipped, bundle-free `render` (a `tag` it accepted but never consulted, "no
  bundle/lookup mechanism") into a real localiser, using mechanisms this
  compiler already had rather than inventing new ones — the multi-file-commons
  merge ([ADR 0160](../decisions/0160-multi-file-commons-test-barrel.md)) for
  "one locale per file, several files, one bundle," and `match` exhaustiveness's
  bounded-structural-coverage shape
  ([ADR 0169](../decisions/0169-nested-payload-patterns-and-match-arm-guards.md))
  for reference-bundle completeness. All three named slices shipped: **1**
  (v0.228.0, [ADR 0272](../decisions/0272-messages-construct-slice-1.md),
  [#859](https://github.com/accuser/bynk/issues/859)) — the `messages <tag>
  @reference { "code" => "template" }` construct as a commons item, a
  `(tag, code) -> template` lookup, and a generated bundle-scoped `render`
  composing with `bynk.locale.render`'s existing floor, given a real
  checker-visible signature via a synthetic function-table entry; **2**
  (v0.229.0, [ADR 0273](../decisions/0273-messages-checked-catalogue-slice-2.md),
  [#874](https://github.com/accuser/bynk/issues/874)) — multi-locale bundles
  actually render (`tag` finally read, not just accepted),
  `bynk.messages.incomplete` (reference-bundle completeness, one diagnostic
  per missing `(locale, code)` witness) and `bynk.messages.placeholder_mismatch`
  (cross-locale template-placeholder *set* agreement, order-insensitive), and
  the exported `messagesLocales`/`messagesReferenceLocale` set that unblocked
  the Locale track's own negotiation slice; **3** (v0.230.0,
  [ADR 0276](../decisions/0276-messages-icu-format-slice-3.md),
  [#878](https://github.com/accuser/bynk/issues/878)) — ICU MessageFormat
  (`plural`/`select`/`number`/`date` placeholders), parsed by a new
  self-contained mini-parser (`bynk-emit/src/emitter/icu.rs`, no
  `bynk-syntax` grammar change) and rendered by delegating to the host `Intl`
  object — no CLDR data bundled in the compiler — plus
  `bynk.messages.format_mismatch` and `bynk.messages.malformed_icu_syntax`.
  Spec-in-place in `design/tracks/message-bundles.md`'s own §4 (now retired
  with the doc; the decisions live on in the ADRs above); surface lives in
  `bynk-emit/src/emitter/emit.rs`, `bynk-emit/src/emitter/icu.rs`, and
  `bynk-emit/src/project/validate.rs`. **Deferred follow-ons, named not
  silently assumed away:** construction-site catalogue checking — does a
  `message(code).withText(...)` builder chain actually supply the parameter
  names its code's reference template declares — has no precedent anywhere
  in this compiler (every existing "declared shape checked at use" mechanism
  keys on an identifier, never a runtime `String` value) and was deliberately
  left unbuilt (§4.3/§7 M1); code identity ships as a bare, unnamespaced
  dotted `String` pending the still-unfiled packaging identity model (§4.5/§7
  M5 — the same gap `deploy.md`'s own retirement summary above names);
  slice 3's own named exclusions — `selectordinal`, `plural`'s `offset:`/`=N`,
  CLDR skeletons beyond a fixed style-keyword set, nested ICU dispatch, and
  construction-site argument-type checking against a code's declared ICU
  usage — each diagnosed rather than silently mishandled, not built. A real,
  pre-existing rough edge surfaced during slice 3 but not fixed: a context
  consuming a different commons' message bundle for its own `render` hits a
  `bynk.uses.name_conflict` if it also needs `bynk.locale`'s
  `message`/`withWhole`-family constructors (both export a symbol named
  `render`) — no fixture across any slice exercises cross-context bundle
  consumption, only a bundle testing its own commons.
- **`locale-capability.md`** — Bynk's first i18n surface: an ambient `Locale`
  capability (`current() -> Effect[LocaleTag]`) paired with a pure, total
  `render(tag, msg) -> String` — the runtime seam a validation message needs
  to become localised text, without touching predicate purity. All three
  named slices resolved: **1** (v0.221.0,
  [ADR 0256](../decisions/0256-locale-capability-slice-1.md),
  [#844](https://github.com/accuser/bynk/issues/844), PR #845) — the
  capability, `LocaleTag`/`Message`/`MessageArg`, and a bundle-free `render`
  (a fixed `"en"` on every platform this slice, `tag` accepted but unused),
  plus the `message`/`withText`/`withWhole`/`withNum`/`withMoment` builder
  API, living in a new firstparty commons `bynk.locale` (a plain `fn` inside
  an `adapter` has no export mechanism, forcing this placement); **2**
  (v0.231.0, [ADR 0277](../decisions/0277-locale-negotiation-slice-2.md),
  [#882](https://github.com/accuser/bynk/issues/882), PR #884) — Cloudflare-
  only real `Accept-Language` negotiation via RFC 4647 basic filtering
  against a context's uniquely-detected message bundle, shipped with a real,
  named limitation: a `uses`-clause name collision meant the shipped wiring
  could never be exercised end-to-end, verified only at the unit level;
  **closed by** (v0.232.0,
  [ADR 0278](../decisions/0278-locale-types-split.md),
  [#886](https://github.com/accuser/bynk/issues/886), PR #888) — splitting
  `bynk.locale` into a dependency-free leaf, `bynk.locale.types`
  (`LocaleTag`/`MessageArg`/`Message`), so a context calling
  `Locale.current()` no longer collides with a message-bundle commons's own
  synthesised `render` — closing the exact rough edge the sibling
  message-bundles track's own retirement summary above names, and verified
  end-to-end this time, not just at the unit level
  (`bynkc/tests/fixtures/positive/817_locale_bundle_wrapper_e2e`); **3** —
  retired in favour of message-bundles' own slice 3
  ([#878](https://github.com/accuser/bynk/issues/878),
  [ADR 0276](../decisions/0276-messages-icu-format-slice-3.md)): ICU
  MessageFormat resolved the ICU/CLDR dependency decision (L4) for both
  tracks. Surface lives in `bynk-check/src/firstparty/bynk.locale.bynk` /
  `bynk.locale.types.bynk`, the three platform bindings
  (`bynk-check/src/firstparty/bindings/bynk-{node,browser,cloudflare}.ts`),
  and `bynk-emit/runtime/src/locale.ts` (`negotiateLocale`). **Deferred
  follow-on, named not silently assumed away — the track's own stated payoff
  never shipped:** spine issue #838 framed this track's payoff as "a
  validation error escaping a boundary reaches the caller in their language
  with no handler code" — automatic boundary-codec integration, turning a
  refinement failure directly into a localised `Message`. That depends on a
  `predicate`-declaration language change (turning `ValidationError.message`
  from a free-text string into a `Message { code, params }` descriptor)
  which was never filed, has no design-notes section, and does not exist as
  of this retirement — every `render` call across all three slices is
  manual, handler-authored. A future track picking this up starts from the
  `predicate`-declaration gap named here, not from `Locale` itself (which is
  complete).
- **`identity-and-totality.md`** — phase 3 of
  [`../bynk-compiler-trajectory.md`](../bynk-compiler-trajectory.md), opened directly by
  `compiler-architecture.md`'s own retirement: node identity independent of position (`ExprId`,
  `FileId`), every side table total (`Ty::Error`, `expr_types`), the checker's core type
  (`Ty`/`TyId`) interned and `Copy`-cheap, and the analysis/emission boundary a type
  (`certify`/`CheckedProgram`) rather than a control-flow convention. Settled 3 August 2026 under a
  same-day review (§3, seven design questions), with one direction reversed twice in the process — first
  toward an `ExprKey(Span)` scaffolding step, then away from it once implementation work on T3.1
  found `43abc242` (a commit predating this track) had already shipped and already rejected that
  exact newtype, on the same grounds. All nine slices shipped: **T3.0** — an existing debug-only
  uniqueness check (finding #28) extended from `check_record`'s top-level-`fn` loop to
  service/agent handler bodies and test-case bodies, which had no collision protection at all;
  **T3.3a** — `Ty::Error` a real variant, given deliberate semantics at every exhaustive match the
  compiler's own exhaustiveness check found (11 real sites, not the 900+ raw mentions a grep
  count suggested); **T3.3b** — `expr_types` made total in full (**R4.3 closed**), at
  `type_of`/`type_of_block`'s two actual write choke points rather than the ~81-function
  internal-recovery-convention rewrite first estimated; **T3.4** — real `ExprId` allocated at parse
  (`Parser::alloc_expr_id`), `expr_types` re-keyed by it (**R2.4 closed for expressions, R2.5/R4.9
  functionally but not structurally** — `expr_types` is `HashMap<ExprId, TypedExpr>`, not the
  `IndexVec<ExprId, TyId>` R2.5/R4.9 literally name; see the open question below), catching and
  fixing a genuine cross-file `ExprId` collision bug (two independently-zero-based files colliding
  once `collect_unit_methods` merged their bodies) before it ever shipped; **T3.5** — real `FileId` on `Span`, stamped at the lexer (the one place
  a `Span` is ever first minted from real bytes) rather than the 160-construction-site conversion
  first estimated, exposing and fixing a latent bug in the LSP rename validator's own span
  reconstruction; **T3.6a** — `Hash`/`Ord` on `Ty`/`NamedKind`/`BaseType`, a 3-line derive diff
  touching zero call sites, closing half of R4.2 without needing any of T3.6b's interning work;
  **T3.7a/T3.7b** — `certify`/`CheckedProgram`, the single-file compile path then the
  project/batch path, closing R3.10 in full; T3.7a's own stated reason for excluding the
  project/batch path (that per-unit emission ran before that unit's build-wide gate was decided)
  was itself re-checked while implementing T3.7b and found to conflate whole-build atomicity
  (already correct, unconditional) with per-unit certification (the real, narrower thing R3.10
  asks for); **T3.6b** — real `Ty` interning, `TyId` the only currency above the intern table,
  landed as the three-stage rewrite (`bynk-check` atomically, then `bynk-emit`, then
  `bynk-ide`/`bynk-lsp`) the settling review predicted — the one slice in the track whose large
  estimate held rather than shrinking, ~250 sites across four crates, tracked to completion as
  its own issue ([#1072](https://github.com/accuser/bynk/issues/1072)) from a WIP branch that had
  the enum conversion done and nothing past it. `span_keyed_maps` reads 3, down from 27 — the
  remainder is `Ctx::pattern_binding_types`, a deliberate exclusion (the `PatId`/`ExprId` split
  the reference draws as out of scope on purpose, not residue left behind). Decisions in ADRs
  [0316](../decisions/0316-ty-interning-interior-mutability.md) (`Types::intern` takes `&self` —
  the table is reached from `Ctx`, whose other fields are `&mut` and routinely live across an
  interning call; a `&mut Types` would have made the borrow checker, not the type system, the
  thing every minting site was written around),
  [0317](../decisions/0317-ty-interning-atomic-not-cell.md) (the table is `Arc`/`Mutex`, not
  `Rc`/`RefCell` — the compiler itself is single-threaded, but the table rides out to
  `bynk-lsp`'s `async` `tower-lsp` handlers, which require `Send`), and
  [0318](../decisions/0318-ty-interning-one-table-per-build.md) (the table is owned per *build*,
  not per `check_record` invocation — a refinement to the settling review's own answer, forced by
  the project path funnelling every unit's `expr_types` into one shared sink, where per-unit ids
  would have been ambiguous). Surface lives in `bynk-check/src/checker.rs` and its
  `calls`/`expressions`/`kernels`/`linearity`/`refinements` submodules, `bynk-emit/src/emitter.rs`
  and `project.rs`, and `bynk-ide`/`bynk-lsp`'s completion/hover/navigation surfaces.
  **T3.6b's own two mistakes, both caught by the test suite rather than review, recorded because
  they are the failure mode interning specifically introduces:** a synthesised `TypedCommons`
  given a *fresh* interner table on the plausible-but-wrong reasoning that its `expr_types` starts
  empty (true at construction, false once case/property checks filled it in against a *different*
  table); and a `compatible` fast path on `TyId` equality that must not exist, since `compatible`
  is not reflexive for sealed boundary values (`Actor`/`ActorSum` are matched, never assigned, so
  even `t == t` must stay `false`). Both now pinned by regression tests; a debug-mode identity
  guard (each `Types` table draws a distinct tag, each `TyId` carries its table's, `resolve`
  compares them) catches a foreign-but-in-range `TyId` a bounds check alone would silently
  mis-resolve. **No unbuilt row in §6's slice table** — all nine named slices shipped, unlike every
  other track retired above. **One open question §6's own completion probe left, named here rather
  than silently dropped with the doc**: `expr_types` is `HashMap<ExprId, TypedExpr>`, functionally
  total (T3.3b/T3.4) but not the `IndexVec<ExprId, TyId>` R2.5/R4.9's literal "IndexVec, i.e. total"
  wording names — closing that structural gap was deliberately left as a question for whoever picks
  the compiler back up next, not decided or filed as residue here, since (unlike T3.6b) nobody has
  yet measured whether the `HashMap` shape is actually a problem worth an `IndexVec` migration to
  fix. What this track opens: phase 4 (the project model as its own crate, `bynk-project`) per
  [`../bynk-compiler-trajectory.md`](../bynk-compiler-trajectory.md), entry-gated on this track's
  own probe (`span_keyed_maps`) reading zero — met, modulo the stated `Ctx::pattern_binding_types`
  exclusion above. Retired 4 August 2026.
- **`content-ownership.md`** — `bynk-lsp` becomes the sole reader of `.bynk`
  project source content, so an unsaved editor buffer is visible everywhere in
  the IDE surface (completion, hover, signature help, go-to-declaration, and
  the published diagnostics round) instead of some paths silently reading
  stale on-disk content via `bynk-emit`'s `read_source` disk-read fallback —
  realising R2.3 for `bynk-emit`/`bynk-ide`, open since T0.7
  ([#1006](https://github.com/accuser/bynk/issues/1006)/[#1012](https://github.com/accuser/bynk/pull/1012)).
  Settled via a re-settling pass (the doc's first merge, #1087, left every §3
  question open despite being merged ready-for-review) that closed §3.1–§3.5
  for real and front-loaded three ADRs (0322–0324). All six slices shipped:
  **0** — `bynk-lsp`'s disk sweep + `for_each_unit` takes content
  ([#1089](https://github.com/accuser/bynk/issues/1089)) — merged from the
  original slices 0+1 under an implementation-time correction: §3.1's
  `ProjectDirs`/`resolve_dirs` design wasn't needed (`bynk_ide::discover_files`
  already closed the gap), recorded as
  [ADR 0325](../decisions/0325-content-ownership-seam-simplification.md),
  superseding [ADR 0322](../decisions/0322-content-ownership-seam-type.md);
  **1** — `symbols.rs`'s cross-file lookups take content, `Backend::project_files`
  retired ([#1092](https://github.com/accuser/bynk/issues/1092)), reaching
  `fs_below_driver=0` for `bynk-ide`; **2** — `AnalysisRoots::lower`'s own
  `bynk.toml` read joins the overlay
  ([#1094](https://github.com/accuser/bynk/issues/1094)); **3** —
  `bynk-testkit`, the cross-crate test-fixture replacement, proved on three of
  its four planned call-site groups
  ([#1096](https://github.com/accuser/bynk/issues/1096)); **4** — the
  remaining ~120 test call sites migrated, sub-sliced by crate across three
  PRs ([#1098](https://github.com/accuser/bynk/issues/1098)); **5** — the
  fallback itself deleted
  ([#1102](https://github.com/accuser/bynk/issues/1102)), surfacing three
  further real dependencies on it beyond what slice 4's migration covered
  (two production `bynk-lsp` request paths built their overlay from open
  buffers only; `AnalysisRoots::lower`/`discover_files`'s own manifest read
  had the identical gap for every manifest-backed caller, moved to callers
  above the driver per R2.3 rather than fixed inside `bynk-ide`; and a
  narrow, permanent, deliberate carve-out for adapter `.binding.ts` reads,
  whose path is only known post-parse and so can never be pre-enumerated the
  way `.bynk` files are) — each fixed in the same PR rather than shipped as
  silent breakage, with a real `Backend`-driven behaviour test
  (`an_unsaved_edit_in_one_file_is_visible_to_completion_in_another`,
  `type_receivers_slow_path_sees_a_closed_files_disk_content`) proving the
  property this track exists for. `fs_below_driver` reads 0 for `bynk-ide`,
  3 for `bynk-emit` (the enumeration walk, the adapter-binding carve-out, and
  the manifest-read carve-out — all now named-and-intentional rather than
  residual, though the probe itself doesn't yet say so structurally; filed as
  [#1104](https://github.com/accuser/bynk/issues/1104), a probe-precision
  follow-on, not reopening any of the three decisions). No new ADR beyond
  0325 above — every other correction found under implementation was
  recorded in the doc and in each slice's own PR, not elevated to a design
  decision. Surface lives in `bynk-emit/src/project/discovery.rs` and
  `paths.rs`, `bynk-ide/src/lib.rs`, `bynk-lsp/src/content.rs` and `lib.rs`,
  and the new `bynk-testkit` crate. Retired 6 August 2026.
- **`project-model.md`** — phase 4 of
  [`../bynk-compiler-trajectory.md`](../bynk-compiler-trajectory.md), opened directly by
  `identity-and-totality.md`'s own retirement: the project model (discovery, the unit graph, the
  schema registry) moved below both `bynk-check` and `bynk-emit` into its own crate, and `bynk-ide`
  repointed at a `bynk-check`-native analysis entry point instead of `bynk-emit`, closing the CI-gated
  dependency-graph check R10.2 asks for. Settled under a same-branch settling review (spine
  [#1107](https://github.com/accuser/bynk/issues/1107), settling PR
  [#1108](https://github.com/accuser/bynk/pull/1108), 6 August 2026) that argued all six of its design
  questions; one (Q3) surfaced a finding the original draft didn't anticipate — `bynk-ide`'s real
  dependency wasn't on a relocatable discovery function but on `run_checks`, an orchestrator that also
  checks — and grew the phase's actual shape past what the draft's relative-size-3 rating assumed.
  Three ADRs front-loaded before slicing:
  [0326](../decisions/0326-project-model-phase4-scope.md) (extract today's name-keyed shape; the typed
  `ProjectGraph`/`UnitId`/`ContractHash` defer to phase 8),
  [0327](../decisions/0327-project-model-symbols-boundary.md) (the module boundary is a five-part test
  plus a composite rule, not "no literal `bynk_check` import" — a check that took four review passes to
  state completely), and
  [0328](../decisions/0328-project-model-analysis-entry-point.md) (the new `bynk-check` entry point
  closes R10.2 without moving `run_checks` itself, accepting temporary, named duplication as phase 5's
  to remove). All three named slices shipped: **P4.0** — the `bynk-project` crate skeleton:
  `discovery.rs`, `graph.rs`, `paths.rs`, `consistency.rs`, `schema_registry.rs`'s
  `SchemaRegistry`/`parse`/`serialize` (not `reconcile`), `AttributedError`
  ([#1113](https://github.com/accuser/bynk/issues/1113)/[#1114](https://github.com/accuser/bynk/pull/1114),
  closing **R3.7, R3.8, R3.9**); **P4.1** — `bynk_check::analysis::analyse_project`, the narrow
  discovery→parse→resolve→check entry point ADR 0328 named, with `symbols.rs` and
  `ProjectAnalysis`/`ContextSequenceInfo`/`ContextBoundaryInfo` relocated to `bynk-check`
  ([#1115](https://github.com/accuser/bynk/issues/1115)/[#1117](https://github.com/accuser/bynk/pull/1117),
  review fixes in [#1119](https://github.com/accuser/bynk/pull/1119)) — two scope corrections found
  under implementation, not deferred: the per-file `check_context_constraints`/
  `check_context_declarations` transitive closure (~3,800 of `validate.rs`'s ~5,000 lines) moved to
  `bynk-check/src/context_checks.rs`, and `run_checks`'s own discovery→group→resolve orchestration
  (~2,070 lines) moved to `bynk-check/src/project_model.rs`/`check_pipeline.rs`; a differential fixture
  (`bynk-check/tests/differential_analysis.rs`) pins the new entry point's diagnostics against
  `run_checks`'s `Mode::Analyse` arm, later widened to pin a seventh residual-gap category
  (test/integration-suite processing) a post-merge review caught mis-filed as orthogonal; **P4.2** —
  `bynk-ide` repointed at the new entry point, all nine `bynk_emit::project`/`bynk_emit::emitter` reach
  points closed, `bynk-ide/Cargo.toml`'s `bynk-emit` dependency line deleted
  ([#1122](https://github.com/accuser/bynk/issues/1122)/[#1123](https://github.com/accuser/bynk/pull/1123),
  closing **R10.2**) — grounding its own regression fixtures found `check_platform_lock` was already
  unreachable from the editor's analysis before this slice too (the editor always analyses as the
  default Cloudflare platform/Bundle target, under which platform-lock can never fire), correcting the
  tracking issue's own six-category count down to five genuine regressions, pinned in
  `bynk-lsp/tests/analysis_residual_gap.rs` and named in the CHANGELOG. §3.5 separately found R3.11
  already closed by prior paydown (#1078), corrected directly in the reference appendix rather than
  cited as a slice's own close. `ide_emit_edge` reads absent — the CI-gated dependency-graph check R10.2
  asked for, already built (T0.0) and already running continuously, needed no new probe to flip. The
  accepted debt this phase names rather than resolves: `run_checks`'s `Mode::Analyse` arm still
  duplicates the new entry point's work, and the editor's project analysis is diagnostically faithful to
  it minus five checks (`messages`-bundle validation, locale-bundle ambiguity, event-subscription
  validation, function-type-boundary checks, and everything inside a `suite`/`test integration` body)
  until phase 5 ports them. What this track opens: phase 5 (semantics centralisation — `validate.rs`
  dissolves into `bynk-check`, `run_checks`'s checking half folds onto P4.1's entry point, deleting the
  duplication ADR 0328 accepted), entry-gated on this track's own probe (`ide_emit_edge`) reading
  absent — met. Surface lives in the new `bynk-project` crate in full, `bynk-check/src/analysis.rs`,
  `context_checks.rs`, `project_model.rs` and `check_pipeline.rs`, and `bynk-ide/src/lib.rs` and its
  `architecture.rs`/`sequence.rs`/`symbols.rs`/`wire_contract.rs`. Retired 8 August 2026.
- **`semantics-in-the-checker.md`** — phase 5 of
  [`../bynk-compiler-trajectory.md`](../bynk-compiler-trajectory.md), opened directly by
  `project-model.md`'s own retirement note above: every remaining whole-project check still living in
  `bynk-emit` relocates to `bynk-check`, so `bynk-check` is the one crate that checks (R3.5) and
  `bynk-emit` originates no diagnostic of its own. Settled under a same-branch settling review (spine
  [#1126](https://github.com/accuser/bynk/issues/1126), settling PR
  [#1127](https://github.com/accuser/bynk/pull/1127), 8 August 2026) that argued all five of its design
  questions; Q2 was the load-bearing one — the emission/checking boundary P4.1 drew informally, without
  its own settling review, turned out to already be a shipped, `CHANGELOG`-named, fixture-pinned live
  regression in the editor's diagnostics (`bynk-check/src/analysis.rs`'s own seven-category residual-gap
  accounting), not an architectural judgment call still open to debate — which settled "all seven
  categories are in scope" with more force than the draft anticipated, sequenced by whether each closes
  a live regression (categories 2, 3, 4, 6, 7) or only architectural compliance (categories 1, 5, both
  already unreachable from the editor before this track and after it, for the same
  `Platform::default()`/`BuildTarget::Bundle`-hardcoding reason `project-model.md` found for
  `check_platform_lock`). Q3 folded into the same finding. Three ADRs front-loaded before slicing:
  [0329](../decisions/0329-semantics-phase5-rule-scope.md) (R4.6, R4.11 and R10.4 stay in scope
  narrowly, as verify-only items — all three already read landed in Appendix D),
  [0330](../decisions/0330-semantics-phase5-check-relocation-scope.md) (all seven categories relocate,
  split by priority not by in/out — the most load-bearing of the three), and
  [0331](../decisions/0331-semantics-phase5-function-boundary-hook.md) (`check_function_type_boundaries`
  moves into `bynk-check`, closing the optional-hook seam `bynk-check` reached back through). All six
  named slices shipped: **P5.0** — `check_messages_bundles`/`check_locale_bundle_ambiguity` relocated as
  `phase_messages_bundles`/`phase_locale_bundle_ambiguity`, closing categories 2–3
  ([#1128](https://github.com/accuser/bynk/issues/1128)/[#1129](https://github.com/accuser/bynk/pull/1129));
  **P5.1** — `check_event_subscriptions` relocated as `phase_event_subscriptions`, closing category 4
  ([#1130](https://github.com/accuser/bynk/issues/1130)/[#1131](https://github.com/accuser/bynk/pull/1131));
  **P5.2** — `check_function_type_boundaries` relocated as `phase_function_type_boundaries`,
  `phase_group`'s optional hook deleted and called directly instead, closing category 6 and ADR 0331
  ([#1132](https://github.com/accuser/bynk/pull/1132)); **P5.3** — `schema_registry::reconcile` relocated
  to a new `bynk-check::schema_registry` module and `check_platform_lock` relocated as
  `phase_platform_lock` on a from-scratch pure resolution walk (not `bynk-emit`'s own TypeScript-building
  helper of that name), closing categories 1 and 5 structurally, not observably
  ([#1133](https://github.com/accuser/bynk/pull/1133)); **P5.4** — `process_tests`/
  `process_integration_tests` split at the check/emit boundary rather than ported whole: the checking half
  (target/participant resolution, `stub` resolution, case/property body type-checking) relocated to a new
  `bynk-check::test_suites` module, TypeScript emission stayed in `bynk-emit::project::tests_emit` as a
  caller of it, closing category 7 (the last and the one `analysis.rs`'s own author flagged as needing
  more care) — go-to-definition/find-references inside a test file restored too, since both relocated
  functions still populate `RefSink`
  ([#1134](https://github.com/accuser/bynk/pull/1134)); **P5.5** — the two sites outside the
  seven-category accounting: `bynk.secrets.computed_name` (a real, `CompileError::new`-constructed
  diagnostic whose own comment claimed it reached the editor — true only until P4.2 repointed `bynk-ide`
  off `run_checks` entirely, stale silently after, named an open risk rather than a scoped item by the
  settling pass) relocated to `bynk_check::secrets::secret_reads_of`, and
  `bynk.project.schema_registry_corrupt` relocated to
  `bynk_check::schema_registry::parse_or_diagnose`; `validate.rs` (empty since P5.3) deleted; `bynk-emit`'s
  crate doc and `Cargo.toml` description corrected to what actually remains — TypeScript emission plus
  `compile_project`/`run_checks`'s per-unit build sequencing (R10.1)
  ([#1135](https://github.com/accuser/bynk/pull/1135)). `emit_diagnostics` moved 49/53 (true/naive, this
  track's own baseline) → 37/41 → 30/34 → 30/34 (P5.2's diagnostics were already `bynk-check`-owned) →
  25/27 → 6/8 → **4/6** — the named floor (§5 of the retired doc): four registered codes remain, all
  `#[cfg(test)]` assertion *strings* in `bynk-emit/src/project.rs`'s own test module, referenced as
  expected output rather than constructed; naive stays above true because two non-diagnostic literals
  (`bynk.locale`, `bynk.toml`) share the `bynk.` prefix without being registered codes. `R4.6`/`R4.11`
  reverified clean throughout — every new `bynk-check` call site reached `ResolvedCommons` through its
  real constructor, no hand-rolled construction. Every category's pin in
  `bynk-lsp/tests/analysis_residual_gap.rs` flipped from a pinned absence to either positive coverage (the
  five live regressions) or a documented stay-absent assertion (the gap-in-name-only sites); the
  differential fixture in `bynk-check/tests/differential_analysis.rs` gained dedicated parity tests along
  the way and needs no further widening — no divergence exists left to prove parity on. Surface lives in
  `bynk-check/src/project_model.rs`, `schema_registry.rs`, `test_suites.rs` and the new `secrets.rs`;
  `bynk-emit/src/project.rs`, `project/schema_registry.rs`, `project/tests_emit.rs` and
  `emitter/secrets.rs` (`project/validate.rs` deleted). Retired 9 August 2026. Opens phase 6 (the IR, per
  the trajectory) — its own reference-doc rationale ("move the checks first and the IR only has to carry
  what emission needs") is exactly the boundary this track drew, category by category, rather than
  leaving as a fuzzy architectural preference for phase 6 to reopen.
- **`the-ir.md`** — phase 6 of [`../bynk-compiler-trajectory.md`](../bynk-compiler-trajectory.md), spine
  [#1137](https://github.com/accuser/bynk/issues/1137): `bynk-emit` gains a typed IR
  (`ir.rs`/`ir/lower.rs`, both landing inside `bynk-emit` rather than as the reference's own separate
  `bynk-ir`/`bynk-lower` crates — [ADR 0332](../decisions/0332-the-ir-crate-location.md), Part 10's
  own crate split deferred to phase 7 for want of a second consumer), and the emitter's own dispatch
  decisions move off re-derived AST name-matching onto reads of a resolved `Callee`/`IrItem`/`TyId`
  value. Settled across three front-loaded design questions
  ([ADR 0332](../decisions/0332-the-ir-crate-location.md) — crate location;
  [ADR 0333](../decisions/0333-the-ir-callee-in-bynk-check.md) — `Callee` classification is new
  `bynk-check` work R6.10 commissions, not phase-5 scope missed;
  [ADR 0334](../decisions/0334-the-ir-lowering-totality-discipline.md) — a certified-program-only
  lowering pass enforces `IrExpr`'s total-by-construction guarantee without needing R4.9's `IndexVec`
  conversion first), then run as three arcs. The **IR construction** arc (P6.0–P6.24, ~25 slices)
  built `Callee` classification, `IrExpr`/`IrItem`/`IrHandler` and their lowering pass, closing R6.5's
  data-loss defect structurally (`body_writes_state` reads `Callee::Store` directly — no
  name-matched receiver, the "strongest single argument" the ADR-0076 trigger check named) along the
  way — landmarks include the `?`/`is` desugars
  ([ADR 0337](../decisions/0337-question-ir-lowering.md)/[ADR 0338](../decisions/0338-is-ir-lowering.md))
  and the `lower_service_item_ir` safety probe reaching zero panics across the whole fixture corpus,
  down from ~51 ([ADR 0347](../decisions/0347-remaining-root-causes-closed.md)). The **completion
  plan** arc (§6a, P6.25–P6.41, its own eighteen slices) made the `ast_importers` probe itself
  false-zero-resistant (a `use super::*;` glob-inheritance hardening a code review caught,
  [#1259](https://github.com/accuser/bynk/pull/1259)) and converted every declaration-read the
  original grounding pass had named, closing at a probe reading of 7 — three files short of the 0
  first named, each for a reason traced rather than assumed, with its own remit explicitly ending
  short of retirement. The **retirement plan** arc (§6b, P6.42–P6.58, seventeen slices) took both
  routes §6a's own hand-off point had named and left untaken: a systematic sweep of
  `project.rs`'s and `emitter.rs`/`emit.rs`/`lower.rs`'s own remaining surface (Phases G/H), then a
  second re-settling (P6.58) arguing the result. `project.rs` cleared entirely — nine slices relocated
  its remaining reads to the `bynk-check`/`bynk-project` crates that already owned the data, or
  re-exported a type from a `bynk-check` module whose own public API was already parameterised by it
  (the [ADR 0352](../decisions/0352-reexport-exprid-from-bynk-check.md)-style `ExprId` precedent, applied four
  more times), landing `ast_importers` at 5 and clearing `project/diagnostics.rs` with it (it rode on
  `project.rs` via the same glob-inheritance rule). Phase H's own conversions closed several
  independent live defects along the way without moving the probe at all — a shadowing hazard in
  cross-context call detection re-derived syntactically instead of reading the checker's own resolved
  `Callee::Cross`, duplicated store-field-annotation walks that could silently diverge, and a
  backwards `Ast ⇄ Ir` dependency where the lowering pass called up into the emitter — while three
  originally-planned conversions (a full `AgentShapeIr`/`ProviderShapeIr` mirror, and a `TypeShape`
  route for two `emitter.rs` predicates) were traced against their own real consumers and declined
  rather than force-built, each for a written reason.

  **`ast_importers` retires at 5, not 0 — the criterion re-settled (P6.58), not missed.** The floor is
  `bynk-emit/src/emitter{,/**}` exactly: `emitter.rs` (~60% of its own remaining AST references
  blocked on three structural facts — no unit-level IR, since every `lower_*_item_ir` function takes
  the AST declaration as its own input; `bynk_check::checker::Ty::Base`'s own `BaseType` parameter
  forcing the AST import into even the fully IR-native `ts_ty`; no `IrExpr` children iterator to
  replace `expr_children`/`statement_exprs`, compounded by an external-reference walk needing the
  source-declared name a resolved `Ty::Named` erases); `emitter/emit.rs` and `emitter/lower.rs`
  (each counted twice over — by their own Q7-settled body-rendering/phase-7-codec residue and by the
  same glob-inheritance rule, independent of either file's own content, for as long as `emitter.rs`
  itself imports the AST); `emitter/workers.rs` and `emitter/workers_entry.rs` (declaration reads
  needing a `TypedCommons` two slices confirmed genuinely isn't in scope at their own call sites).
  `AST_IMPORTER_EXCEPTIONS` did not grow to reach this floor at any point in either arc. Breaks a
  circularity neither the original completion criterion nor phase 7's own forward-reference had named:
  phase 7's `bynk-ts` printer's entry condition was "this track's probe reads 0," but ~52 references in
  `emitter.rs` alone are `bynk-ts`'s own work by the codec re-settling's ruling
  ([ADR 0358](../decisions/0358-codec-layer-resettling.md)) — one shared predicate-message mapping is
  literally half-consumed by the already-excluded codec renderer — so the renderer family could never
  leave before `bynk-ts` existed to receive it, and `bynk-ts` could never start under the old wording
  until the renderer family left. `bynk-ts`'s own entry condition now reads the named floor plus that
  boundary, not a literal zero this phase's own scope could never have reached.

  Surface lives in `bynk-emit/src/ir.rs`, `ir/lower.rs` (both new), `emitter.rs`, `emitter/emit.rs`,
  `emitter/lower.rs`, `emitter/workers.rs`, `emitter/workers_entry.rs` (the five files the floor names),
  `project.rs` (cleared), `bynk-check/src/checker.rs` (`Callee`), `resolver.rs`, `contract.rs`,
  `actors.rs`, `project_model.rs`, and `bynk-project/src/discovery.rs`. Fifty-one ADRs
  (0332–0382) carry its decisions in full; `xtask/src/greenfield_status.rs`'s own `ast_importers`
  probe stays in the tree, gated, reading 5 — not deleted at retirement, a regression ratchet phase 7
  inherits and drives down as it builds the printer this floor's own residue names. **Deferred follow-ons,
  named rather than left implicit** (the track's own §7, "Forward references," carries the full
  entry-condition table): the `bynk-ts` tree and printer itself, gated on this floor
  (phase 7); carving `bynk-ir`/`bynk-lower` as their own crates once `bynk-ts` gives the IR a second
  consumer (phase 7); severing `bynk-emit`'s remaining `bynk-check` dependency (phase 7 or later);
  a cross-unit `CheckedProgram` persistence layer — the real prerequisite for a full
  `IrItem::Agent`/`Provider` enumerator `project.rs`'s own compose-time wiring still wants — named
  *unopened, no trigger yet*, not scoped; `Question`'s own three-way desugar fork for R5.9 (unproposed);
  three narrowly-scoped `TypedCommons`-only helpers for `write_header`'s own remaining
  declaration-content checks, and a full `AgentShapeIr` beyond the store-field-kind dedup P6.53 landed
  (both unproposed, and may turn out to be the same helper). Retired 19 August 2026. Opens phase 7 (the
  printer, per the trajectory) inheriting a named, argued rendering-subtree boundary instead of a
  probe reading zero with no map of what's inside it.

- **`the-typescript-tree.md`** — phase 7 of [`../bynk-compiler-trajectory.md`](../bynk-compiler-trajectory.md),
  spine [#1293](https://github.com/accuser/bynk/issues/1293): a new `bynk-ts` crate (TS tree, printer,
  source map) becomes the single writer of every character of generated TypeScript — `bynk-emit`
  builds nodes, `bynk-ts` prints them — inheriting phase 6's own named boundary (`emitter.rs`/
  `emitter/emit.rs`/`emitter/lower.rs`/`emitter/workers.rs`/`emitter/workers_entry.rs`, floor 5)
  instead of a probe reading zero with no map of what was inside it. Settled across four
  front-loaded design questions
  ([ADR 0385](../decisions/0385-typescript-tree-crate-carve-timing.md) — `bynk-ts` carved as a
  crate in the first slice, not built in-module and carved later, since carving it also
  manufactures the second IR consumer ADR 0332 deferred the `bynk-ir`/`bynk-lower` split on;
  [ADR 0386](../decisions/0386-typescript-tree-verbatim-hatch.md) — the migration escape hatch is
  a statement-level `Verbatim` node with a closed `VerbatimOrigin` enum and a companion textual
  lint, since a byte-golden fixture alone can't verify what an opaque block hides;
  [ADR 0387](../decisions/0387-typescript-tree-any-elimination-scope.md) — `TsType::Any` is
  eliminated in full, a 2–3-site residual named and deferred to R7.7's runtime-typing work rather
  than re-opening phase 6's IR;
  [ADR 0388](../decisions/0388-typescript-tree-r8-scope.md) — R8.1–R8.22 splits four ways: twelve
  rules already closed, five closing as a byproduct of this track's own conversion, two (R8.2,
  R8.14) getting named slices, one shared with phase 8), then run as six arcs. **Arc A** (5
  slices, P7.0–P7.4, independent of the tree) landed the `ts_writes`/`ts_any` gated probes
  ([#1296](https://github.com/accuser/bynk/issues/1296)), verified R7.7 already closed, narrowed
  `ts_any` 55 → 31, and gave `wrangler.toml` its own `TomlDocument`/printer, closing R7.6/R8.20.
  **Arc B** (5 slices, P7.5–P7.9) carved the `bynk-ts` crate per ADR-A, replaced `CompiledFile`
  with a typed `Artefacts`/`Document` (R7.8), named the printer's existing readability guarantee
  (R7.5, partial), and built the first real node algebra (`TsStmt`/`TsExpr`/`TsType`/`TsDecl`)
  against `events_fanout.rs`'s own concrete shape before converting the `ts_type_ref`/`ts_ty`
  type-building family (P7.9) to build real `TsType` internally instead of hand-`format!`-ing
  text. **Arc D** (P7.d1–P7.d3, the R8 rule closures) carved `bynk-ir`/`bynk-lower` out of
  `bynk-emit` with no code deleted — the split ADR 0332 deferred in phase 6 for want of a second
  consumer, landing once `bynk-ts` supplied one
  ([ADR 0392](../decisions/0392-p7d1-ir-lower-crate-carve.md)) — unified `bynk-check`'s and
  `bynk-emit`'s independently-maintained brand-predicate copies
  ([ADR 0393](../decisions/0393-p7d2-brand-predicate-unification.md)), and revisited P6.56's
  declined JSON-codec-seed attempt for real, now that it could read `Callee::Intrinsic`
  ([ADR 0394](../decisions/0394-p7d3-json-codec-seed-callee.md)). **Arc C** (the bulk of the
  track, dozens of slices) converted `bynk-emit`'s own emission call sites to real `bynk-ts` trees
  file by file — `contracts.rs`/`secrets.rs`/`runtime_use.rs`, Arc C's own originally-named
  first-slice trio, turned out not to need conversion at all (they build JSON or aren't emission
  code, found by P7.8/[#1313](https://github.com/accuser/bynk/issues/1313), a correction rather
  than an error since the JSON/TS distinction was invisible before Arc B's own `Document` type
  existed) — `events_fanout.rs`, `workers.rs`, `workers_entry.rs`, `project.rs`'s `emit_test_main`/
  `emit_composition_root` and its per-item dispatch, `emitter/emit.rs`'s `emit_free_fn`/
  `emit_provider`/`emit_service`/`emit_agent`/`emit_stub_class`, and `project/tests_emit.rs`'s own
  wrapper-function family all converted, `lower.rs`'s per-splice-point opaque output staying a
  deliberate, permanent exclusion throughout
  ([ADR 0391](../decisions/0391-arc-c-lower-rs-permanent-exclusion.md)). **Arc E** (7 slices)
  converted `emitter/serialisation.rs` in four bounded clusters plus two caller-side wrappers
  ([ADR 0398](../decisions/0398-arc-e-serialisation-rs-conversion-decomposition.md)). **Arc F and
  the closing floor-arguing arc** ([#1449](https://github.com/accuser/bynk/issues/1449)–
  [#1502](https://github.com/accuser/bynk/issues/1502)) resolved every named residual rather than
  leaving it open-ended: `pred_condition_and_message` converted for real once two missing
  `TsBinaryOp` variants (`>=`/`<=`) were added, the same "add the operator, don't wall it off"
  precedent `LessThan`/`InstanceOf`/`In` each already used; `inject_runtime_imports` turned out to
  be a plain construction-order fix, not entangled with the source-map question it was first
  thought to depend on; and the `verbatim_sites` capstone
  ([#1486](https://github.com/accuser/bynk/issues/1486)) converted `emit_project`/
  `emit_test_module`/`emit_integration_module` — the three orchestrator functions that stayed
  `String`-typed at their own top level regardless of how completely their internals converted —
  to return real trees printed once at the write boundary.

  **All four gated probes retire at an argued floor, not the flat zero §5 first proposed — the
  same honesty `ast_importers` (floor 5, not 0) already modelled for phase 6.** `ts_any` retires
  at **26**
  ([ADR 0404](../decisions/0404-ts-any-residual-six-families.md),
  [#1459](https://github.com/accuser/bynk/issues/1459)/[#1460](https://github.com/accuser/bynk/issues/1460)):
  26 sites across seven files collapse into six already-(or newly-)argued families (collection-
  kernel element types, the Durable Object stub, cross-context dispatch casts, and R7.7's own
  runtime-error-type residual); no site among them turned out newly tractable. `verbatim_sites`
  retires at **2**
  ([ADR 0399](../decisions/0399-ts-any-verbatim-sites-floor-correction.md)/
  [ADR 0407](../decisions/0407-verbatim-sites-capstone-callee-cascade-and-nested-checkpoint.md),
  confirmed unchanged by the #1486 capstone): `project.rs`'s adapter-binding copy loop (an
  adapter binding's own foreign, user-authored TypeScript, copied in verbatim so `compose`'s
  import resolves and the `tsc` gate checks the `implements` contract) and its `runtime.ts`
  staging (a committed npm build artifact — `emitter::emit_runtime_module()`'s own return
  value), neither ever generated by `bynk-emit`. `ts_writes` retires at **809**
  ([ADR 0409](../decisions/0409-ts-writes-final-floor-after-1486.md),
  [#1501](https://github.com/accuser/bynk/issues/1501)): 614 permanent and individually argued
  (`lower.rs`'s 371 under ADR 0391; the four Decision-C hand-written class wrappers
  `emit_provider`/`emit_service`/`emit_agent`/`emit_stub_class`, 126 together, each building its
  own wrapper into a local buffer and wrapping the whole thing as one `TsStmt::raw`;
  `emit_contract_guarded_body`'s 8 message-text-and-source-map-splice-entangled sites;
  `workers_entry.rs`'s and `tests_emit.rs`'s already-read-end-to-end 106
  ([ADR 0408](../decisions/0408-ts-writes-sampled-files-end-to-end.md)); `__eventsDispatch`'s
  opaque carve-out, 3), 190 a newly-named permanent structural category (identifier/type-name/
  message-text `String` construction feeding an already-real node's leaf field, the same
  representational choice P7.9 already made explicit for `TsType::Named`'s pre-rendered text), and
  5 real, small, tractable sites named but not scheduled (a bare blank-line `writeln!` in a
  handful of functions not yet promoted to their own top-level `Vec<TsStmt>` signature).
  `verbatim_origins` retires at **1**
  ([ADR 0410](../decisions/0410-verbatim-origins-floor-at-retirement.md),
  [#1502](https://github.com/accuser/bynk/issues/1502)): only the `NotYetConverted` variant has a
  live reference anywhere in `bynk-emit/src`, at the identical two sites `verbatim_sites` already
  names permanent; `Contracts`/`Secrets`/`RuntimeUse` are dead in production (only `bynk-ts`'s own
  unit-test fixtures still reference them), kept rather than removed per this track's own §9
  "Risks" precedent — orthogonal, later cleanup, not a retirement blocker.

  Surface lives in `bynk-ts/src/{lib,program,printer,source_map,lint}.rs` (the new crate, ~9,500
  LOC), `bynk-emit/src/emitter.rs`, `emitter/{emit,lower,workers,workers_entry,serialisation,
  events_fanout,toml_doc,wrangler}.rs`, `project.rs`, `project/tests_emit.rs`, the carved
  `bynk-ir`/`bynk-lower` crates, and `xtask/src/greenfield_status.rs` (four gated probes —
  `ts_writes`, `ts_any`, `verbatim_origins`, `verbatim_sites` — stay in the tree, not deleted at
  retirement, the same regression-ratchet precedent `ast_importers` set for phase 6). Twenty-one
  ADRs (0385–0410, minus five numbers — 0396, 0397, 0400, 0401, 0402 — that belong to a
  concurrent, unrelated property-generator track) carry this track's own decisions in full.
  **Deferred follow-ons, named rather than left implicit:** incrementality — query granularity,
  `UnitSignature`, the query firewall — gated on exactly the four probes this retirement settles
  (phase 8); R8.16's deferred data-model half, a typed `ProjectGraph`, named by phase 4's own
  retirement note (phase 8); a further crate re-graph beyond `bynk-ts`/`bynk-ir`/`bynk-lower`,
  e.g. R10.5's `bynk-driver` consolidation (named in the reference, *unopened, no trigger yet*);
  removing the three dead `VerbatimOrigin` variants (§9, separate cleanup); the small `ts_writes`
  bucket-C residual, 5 sites (§5/#1501, named but not scheduled). Retired 29 August 2026. Per the
  trajectory's own "a phase's track opens when the previous phase's probe reads zero" rule, this
  retirement is what makes phase 8 openable — but §10 ("What this phase causes") already names
  phase 8 as needing phases 3 and 4 *together with* this one, and its own settling review still
  needs to ground a fresh spine issue against the current tree, the same way this track's own
  opening did against phase 6's, rather than assuming the trigger alone is sufficient.
- **`incrementality.md`** — phase 8 of [`../bynk-compiler-trajectory.md`](../bynk-compiler-trajectory.md),
  spine [#1507](https://github.com/accuser/bynk/issues/1507), **the trajectory's last phase**: every
  compiler output decomposed to the granularity at which it is invalidated —
  `Tokens(FileId)`/`Ast(FileId)` at file level, `UnitSignature(UnitId)` at unit level (declarations
  only, no bodies), `Body(DefId)`/`TypeOf(DefId)` at definition level, `ProjectGraph` at project
  level — with `UnitSignature` proved stable under any edit to a body inside that unit (the R3.14
  firewall). The scheduler that would actually memoise these queries (R3.15) is a separable
  decision this phase commits the granularity for and explicitly defers, per Q3 below.

  Settled across five design questions under review on the settling branch
  ([ADR 0412](../decisions/0412-incrementality-unit-signature-shape.md)–
  [ADR 0414](../decisions/0414-incrementality-no-memo-table-this-phase.md)). Three (Q1, Q2, Q4) each
  turned on a concrete fact the draft hadn't yet checked, narrowing the option space rather than
  reversing the draft's own leaning: **Q1** found no existing type in the workspace is already
  signature-shaped enough to widen — ADR 0200's `combined_types_for` computes only one of design
  notes §15's four required-annotation categories, and every closer candidate (`UnitTable`'s own
  `FnDecl`/`Handler`/`StoreField` types) carries a full body or initialiser — so `UnitSignature` is
  a new type wrapping `combined_types_for`'s existing output unchanged, plus fresh projections read
  from `UnitTable`, compared through a canonical, span/trivia/documentation-erased rendering
  extending ADR 0200's own `canon_type`/`service_normal_form` rather than a new erasure scheme
  ([ADR 0412](../decisions/0412-incrementality-unit-signature-shape.md)). **Q2** found sharing
  completion's own `PROJECT_UNIT_CACHE` with the diagnostics path was never actually available —
  `bynk-check` cannot depend on `bynk-ide`, the crate graph runs the other way — so the settled
  answer builds one new, shared `Tokens(FileId)`/`Ast(FileId)` cache in `bynk-project` instead, plus
  the durable path↔`FileId` interning table it needs, since `FileId` was reallocated fresh on every
  `phase_parse` call rather than interned across them
  ([ADR 0413](../decisions/0413-incrementality-shared-file-level-cache.md)). **Q3** settled that this
  track builds no memo table of any kind — R3.15's own scheduler decision defers whole, on the
  reasoning that its own trigger ("a hand-rolled table measurably becoming the bottleneck") cannot
  fire before this phase's own granularity exists to be a bottleneck in
  ([ADR 0414](../decisions/0414-incrementality-no-memo-table-this-phase.md)). **Q4** audited
  `UnitSignature`'s field list field-by-field against the real `FnDecl`/`Handler`/`StoreField`/
  `ProviderDecl`/`ServiceDecl` shapes, excluding every body or body-adjacent field
  (`requires`/`ensures`, `StoreField.init`/`.annotations`) and — found only once P8.2's own fixture
  was reasoned through — every field's own `Span`/`trivia`/`documentation`, which shift under an
  edit elsewhere in the same file even when nothing semantically relevant changed. **Q5** settled the
  gated probe's own shape as a one-time existence-and-proof check, not a shrinking count — the first
  probe on this trajectory shaped as a proof rather than a floor.

  All six settled slices shipped: **P8.0** ([#1510](https://github.com/accuser/bynk/issues/1510)) —
  the `incremental_query_types`/`keystroke_latency` probes themselves; **P8.1**
  ([#1517](https://github.com/accuser/bynk/pull/1517)) — `UnitId`/`UnitSignature` in `bynk-check`;
  **P8.2** ([#1518](https://github.com/accuser/bynk/pull/1518)) — the property test proving
  `UnitSignature`'s canonical rendering is stable under a body-only edit; **P8.3**
  ([#1519](https://github.com/accuser/bynk/pull/1519)) — a typed `ProjectGraph`, landing in
  `bynk-check` beside `UnitId` rather than in `bynk-project` as the track doc originally assumed,
  since `bynk-project` cannot depend on `bynk-check`'s own `UnitId`
  ([ADR 0415](../decisions/0415-p8-3-project-graph-shape-and-placement.md)); **P8.4**
  ([#1520](https://github.com/accuser/bynk/pull/1520)) — the durable path↔`FileId` interning table
  and the shared parse cache Q2 specified, plus two real forks the settling review didn't examine:
  `ExprId` allocation needed the identical durability treatment as `FileId` (a cached file's stale
  `ExprId`s could collide with a freshly-parsed sibling file's fresh ones), and the shared cache
  stores the strict parse only, not completion's own recovery-tolerant parse, since the two are
  genuinely different parser configurations
  ([ADR 0416](../decisions/0416-p8-4-durable-parse-cache-expr-id-and-strict-vs-recovery.md));
  **P8.5** ([#1521](https://github.com/accuser/bynk/pull/1521)) — `DefId`/`Body(DefId)`/
  `TypeOf(DefId)` in `bynk-check`, split into `DefId::Fn`/`DefId::Handler`/`DefId::ProviderOp`
  (a provider op turned out to need its own identity variant — reusing `HandlerDefId`'s shape let
  two ops of the same provider collide on one key, caught in this PR's own review) and built with a
  fresh `CheckSinks` per call, not wired into any production check path
  ([ADR 0417](../decisions/0417-p8-5-defid-split-and-fresh-sinks.md)).

  **`incremental_query_types` retires at *satisfied*, not a floor** — the first gated probe on this
  trajectory shaped as a proof rather than a shrinking count: `UnitSignature`, `ProjectGraph`,
  `Body`/`TypeOf` all exist as real code in `bynk-check`, the shared file-level cache is migrated
  (`PROJECT_UNIT_CACHE` deleted), and P8.2's own stability test is present. `keystroke_latency`
  (trend-only) stays "not measured," per Q3/Q5's own settled scope — no scheduler ships in this
  phase to produce a real number, and none is owed by this phase's own completion criterion.

  Six ADRs, 0412–0417, carry every decision in full — the first three (0412–0414) front-loaded at
  the settling PR merge, the remaining three (0415–0417) each recording a real fork an implementing
  PR's own review found that the settling review had not examined, the same "flag the fork, don't
  silently absorb it" discipline every prior track on this trajectory has applied.
  **Deferred follow-ons, named rather than left implicit:** R3.15's scheduler decision (salsa or a
  hand-rolled memo table — *unopened, no trigger yet*: needs a hand-rolled table measurably
  becoming the bottleneck, which cannot fire before this phase's own granularity exists to be a
  bottleneck in — tracked as [#1523](https://github.com/accuser/bynk/issues/1523)); R10.5's
  `bynk-driver` consolidation (*unopened, no trigger yet* —
  [#1525](https://github.com/accuser/bynk/issues/1525)); a lossless CST (rowan) (*unopened, no
  trigger yet*: needs a real per-file reparse timing measured as costly, which P8.4's own interning
  table is the first place such a timing could be collected, but collecting it was not this phase's
  job — [#1524](https://github.com/accuser/bynk/issues/1524)); an emit-side
  `UnitSignature`-equivalent keyed on `Artefacts` (R7.8) (*unopened, no trigger yet* — Q1's own
  settled scope covers the check side only — [#1526](https://github.com/accuser/bynk/issues/1526)).
  Retired 30 August 2026.

  **Postscript (2 September 2026, [#1537](https://github.com/accuser/bynk/issues/1537)):** the 30
  August post-restructuring review found that of the four levels above, only P8.4's shared cache had
  a production consumer and only P8.2's test read `UnitSignature`; `ProjectGraph` (P8.3) and
  `Body`/`TypeOf` (P8.5) — 990 lines — were reachable from their own tests alone, with no scheduler
  to call them and, per R3.15/#1523, no trigger yet for one. Both were deleted, the same P5 decision
  the IR cutover (#1542) reached for phase 6's expression IR; `UnitSignature` and its stability test
  stay as the R3.14 proof #1523's trigger presupposes. `incremental_query_types` was re-settled to
  certify the decision in both directions — the surviving levels present, the deleted ones absent —
  so re-adding either without a consumer changes the committed reading. ADR 0415 and ADR 0417 stand
  as the record of what was built; the ADR #1537's own PR carries supersedes their "landed" status.

  **This is the trajectory's last phase.** Per `bynk-compiler-trajectory.md` §1, its endpoint — "the
  compiler Bynk ships today, feature for feature, rebuilt on the architecture in
  `bynk-greenfield-compiler.md`" — is reached at this retirement, not before. This retirement PR
  therefore closes `../bynk-compiler-trajectory.md` itself alongside this track and its spine, per
  that document's own §1 and this track's own §12 — the one retirement on this trajectory that
  closes two documents, not one. What the trajectory's own eight phases do not close, named so a
  future reader does not mistake silence for completeness: R3.15's scheduler decision, R10.5's
  `bynk-driver` consolidation, rowan's lossless-CST question, and an emit-side signature concept for
  `Artefacts` — each a real, later decision with its own trigger, not inherited from any phase that
  came before it.

- **`the-ir-cutover.md`** — the follow-on to phase 6 of
  [`../bynk-compiler-trajectory.md`](../bynk-compiler-trajectory.md), spine
  [#1542](https://github.com/accuser/bynk/issues/1542): opened 30 August 2026 (settling PR
  [#1543](https://github.com/accuser/bynk/pull/1543)) to execute the "cutover" `the-ir.md` settled as
  its own Q7/#1175 but never sliced — routing `bynk-emit/src/emitter/lower.rs`'s dispatch *reads*
  through `bynk-lower`'s fifteen then-unconsumed entry points and `bynk-ir`'s twenty-one unconsumed
  types, after the 30 August post-restructuring review
  ([`../reviews/2026-08-30-post-restructuring-review.md`](../reviews/2026-08-30-post-restructuring-review.md))
  found phase 6 had built an IR the emitter never consumed. **Retired 2 September 2026 on the opposite
  decision to the one it opened with: the cutover stops, and the unconsumed lowering is deleted.**

  What shipped as planned: **Slice 1** ([#1556](https://github.com/accuser/bynk/issues/1556)/
  [#1558](https://github.com/accuser/bynk/pull/1558), [ADR 0418](../decisions/0418-un-defer-http-method-rendering.md))
  closed the four build-then-discard detours the review named and the `HttpMethod` rendering surface
  ADR 0355 had deferred; **Slice 2** was found already satisfied (both "unconsumed" signature helpers
  had a same-crate production wrapper); **Slice 3.1** ([#1564](https://github.com/accuser/bynk/issues/1564)/
  [#1565](https://github.com/accuser/bynk/pull/1565)) closed `bynk-lower`'s two live `todo!()` gaps.

  What did not: **Slice 3.2**, the expr/stmt-core cutover, was accepted on two premises — a mechanical,
  byte-identical retype, landing as one slice so the lowering family never existed in two copies —
  and its own branch (`archive/slice3-2-expr-stmt-core`, 19 commits, never merged) falsified both.
  Against `main`: `emitter/lower.rs` 6,321 → 10,117 lines; 56 `_v2` sibling functions duplicating
  AST-typed ones, none of which was retyped or deleted; a per-body static gate choosing between the two
  paths at each of the 3 of 7 flipped entry points; 6 production `todo!()`s; `ts_writes` 809 → 1,079;
  two goldens accepted as "structurally unrecoverable" diffs; seven follow-on issues for parity gaps.
  Every flip found a real behavioural difference between the paths, each a bug in the IR path, and the
  residual diffs needed `bynk-ir` widened. That is a second code generator being brought to parity with
  the first, gated per body — `bynk-compiler-trajectory.md` §6's question 3 and P5, reproduced by the
  track opened to close them. The re-settling
  ([#1573](https://github.com/accuser/bynk/pull/1573), the track doc's own §10) priced finishing to
  parity (not less than the cost already paid, open-ended, for an end state that still emits strings)
  against deletion, and chose deletion per the trajectory's own §8: "a phase's estimate is wrong by a
  large factor … the phase boundary is the stopping point."

  **A finding the review missed, found while pricing:** the expression lowerer *was* reachable in
  production on `main` — through `lower_event_subscriber_shapes_ir → lower_service_item_ir →
  lower_service_handler_ir → lower_block_ir → lower_expr_ir`, for every `from Events(E)` service,
  lowering every handler body and keeping two booleans. The review's "zero callers" counted direct
  callers only. `lower_ident_ir`'s terminal `unreachable!()` was live on that path with a safety
  argument scoped to a different caller.

  The four D-slices then executed the decision, `rustc` as the worklist rather than the inventory:
  **D0** ([#1574](https://github.com/accuser/bynk/issues/1574)/[#1575](https://github.com/accuser/bynk/pull/1575))
  repointed the detour at the two shape-only helpers it actually needed, zero-diff by construction;
  **D1** ([#1576](https://github.com/accuser/bynk/issues/1576)/[#1577](https://github.com/accuser/bynk/pull/1577))
  deleted 48 `bynk-lower` functions, four `LowerIrCtx` fields and their methods, and every `todo!()` the
  crate carried — `bynk-lower/src/lib.rs` 10,195 → 2,123 lines — finding 121 of 134 tests went (not the
  ≤73 estimated: 51 reached the lowerer through a shared helper) and re-creating the twenty-one that had
  pinned a *kept* helper only through a deleted constructor; **D2**
  ([#1578](https://github.com/accuser/bynk/issues/1578)/[#1579](https://github.com/accuser/bynk/pull/1579))
  deleted 22 `bynk-ir` items — `bynk-ir/src/lib.rs` 1,923 → 634 lines — finding `EmbedIr` had an
  in-crate consumer and `StoreFieldIr::init` was `IrExpr`'s last use (the field went instead), and
  rewrote the crate doc and every surviving doc comment that narrated a deleted item; **D3**
  ([#1580](https://github.com/accuser/bynk/issues/1580), this retirement) recorded the refusal in
  `bynk-greenfield-compiler.md` Part 15.1's four-field form, added the gated `unconsumed_ir_items`
  adoption probe the review's Part 5 §8 asked for, and closed the spine. The probe earned its place
  before it merged: its first run found two more `pub` items with no reader outside their crate
  (`EmbedIr`, `LowerIrCtx`); its own review then found it let the two IR crates vouch for each other —
  run against pre-D0 `main` it would have missed every `bynk-ir` type, since `bynk-lower`'s own
  unconsumed constructors named them — and, once both owners were excluded, five more (`IndexIr` and
  the four `MUTATING_*_OPS` tables, read only from `bynk-lower`). All resolved by inlining the two
  aliases, demoting the struct and moving the tables beside their one reader, never by arguing a
  floor: the final accounting is 47 `bynk-ir` public items = 23 deleted + 2 inlined + 4 relocated +
  1 demoted-in-`bynk-lower`-territory aside, **19 kept**, each with a reader outside both crates.

  **What stays declined, named rather than left implicit:** ADR 0381's six conversions and ADR 0366's
  `TypeShape::Refined` AST embedding (unchanged from the track's own §2); `emit_worker_compose`'s
  `Message`-arm protocol check (ADR 0355, reconfirmed for a stronger reason — the per-unit
  `CheckedProgram` drop); R5.9's is-binding scopes (ADR 0338), moot with the lowering that needed them;
  the seven Slice 3.2 satellite issues ([#1566](https://github.com/accuser/bynk/issues/1566)–[#1572](https://github.com/accuser/bynk/issues/1572)),
  closed as superseded. Phase 6's own retirement floor (`ast_importers` = 5) and the trajectory's §1
  endpoint are not reopened. Phase 8's twin decision — the unadopted query layer,
  [#1537](https://github.com/accuser/bynk/issues/1537) — is the same question on different evidence
  and is not decided here. Two ADRs carry the decisions: ADR 0418 (Slice 1) and the retirement ADR this
  slice adds, superseding `the-ir.md`'s Q7 and ADR 0338.

- **`runtime-semantics.md`** — spine [#1648](https://github.com/accuser/bynk/issues/1648), opened from the
  [2026-10-01 language-implementation review](../reviews/2026-10-01-language-implementation-review.md)
  ([#1647](https://github.com/accuser/bynk/pull/1647)). The review found that the gates certified "the
  output type-checks and matches its golden", not "the program behaves as the spec says". Goldens had
  blessed an agent-state reload fault for months. The track's theme was that emitted programs do what the
  spec says, and that the gates check it. Settled 1 October 2026
  ([#1681](https://github.com/accuser/bynk/pull/1681), v0.290.1); every slice shipped by 4 October
  (v0.290.2–v0.302.0).

  **The four settled questions, each with its ADR.**
  - **`==` is structural**, and defined only on equality-supporting types (S3,
    [#1652](https://github.com/accuser/bynk/issues/1652), ADR 0421). It shipped with or before the
    state-shape slice, as §3.1 required: once state decodes into fresh objects, a reference `==` made an
    invariant like `status == Paid implies …` silently never fire.
  - **Agent state is stored in the wire shape** and decoded on load, with no tolerant loader (S0,
    [#1649](https://github.com/accuser/bynk/issues/1649), the track's P0, ADR 0422). Both S3 and S0 shipped
    in [#1687](https://github.com/accuser/bynk/pull/1687), v0.291.0.
  - **`Matches` patterns are admitted by an automaton ambiguity check** (S2,
    [#1651](https://github.com/accuser/bynk/issues/1651), ADR 0423), with `String.replace` inserting
    literally (S1, [#1650](https://github.com/accuser/bynk/issues/1650)). Both shipped in
    [#1689](https://github.com/accuser/bynk/pull/1689), v0.292.0.
  - **`Int` is the JS safe-integer domain**, enforced at every entry (S8,
    [#1657](https://github.com/accuser/bynk/issues/1657), ADR 0425,
    [#1693](https://github.com/accuser/bynk/pull/1693), v0.295.0).

  **The other correctness slices.**
  - **S4** identifier hygiene ([#1653](https://github.com/accuser/bynk/issues/1653), ADR 0428) and **S7**
    unreachable match arms ([#1656](https://github.com/accuser/bynk/issues/1656), ADR 0429), both in
    [#1698](https://github.com/accuser/bynk/pull/1698), v0.299.0.
  - **S5** `is` bindings across `&&` and a negated `else`
    ([#1654](https://github.com/accuser/bynk/issues/1654), ADR 0427) and **S6** tsc-clean workers builds
    ([#1655](https://github.com/accuser/bynk/issues/1655)), both in
    [#1695](https://github.com/accuser/bynk/pull/1695), v0.297.0.
  - **S9** an unbound `Effect` is an error ([#1658](https://github.com/accuser/bynk/issues/1658),
    ADR 0426), in [#1694](https://github.com/accuser/bynk/pull/1694), v0.296.0.
  - **S10** inference and suite-diagnostic locations
    ([#1659](https://github.com/accuser/bynk/issues/1659)), in
    [#1696](https://github.com/accuser/bynk/pull/1696), v0.298.0.
  - Found while settling:
    - **S11** workers agent calls use the boundary codec
      ([#1678](https://github.com/accuser/bynk/issues/1678), ADR 0424,
      [#1692](https://github.com/accuser/bynk/pull/1692), v0.294.0);
    - **S12** handler signature types must resolve
      ([#1679](https://github.com/accuser/bynk/issues/1679), #1694);
    - **S13** store keys must be keyable ([#1680](https://github.com/accuser/bynk/issues/1680), #1696).
  - Found in review:
    - **S14** store collection keys are own properties
      ([#1685](https://github.com/accuser/bynk/issues/1685),
      [#1690](https://github.com/accuser/bynk/pull/1690), v0.293.0);
    - **S15** a generic function's comparisons bound its callers' type arguments
      ([#1688](https://github.com/accuser/bynk/issues/1688), ADR 0430), with
      [#1700](https://github.com/accuser/bynk/issues/1700), visibility rules holding in handler bodies.
      Both shipped in [#1702](https://github.com/accuser/bynk/pull/1702), v0.300.0.
  - **#291** platform capabilities under `bynkc test`, re-scoped onto the track
    ([#291](https://github.com/accuser/bynk/issues/291), ADR 0431,
    [#1705](https://github.com/accuser/bynk/pull/1705), v0.301.0):
    - deterministic doubles;
    - per-operation `stub`;
    - case-scoped stubs that no longer leak;
    - every example's suite run in CI.

    Its follow-on was a context's optional brand on a `uses`-commons type
    ([#1704](https://github.com/accuser/bynk/issues/1704), ADR 0432,
    [#1707](https://github.com/accuser/bynk/pull/1707), v0.301.1).

  **The gates.**
  - **G0** behavioural fixtures ([#1660](https://github.com/accuser/bynk/issues/1660),
    [#1683](https://github.com/accuser/bynk/pull/1683), v0.290.2). A positive fixture's `expected_run.txt`
    runs its suites through the real `bynkc test`, strictly in both directions. `InMemoryStorage` now
    clones, so `bynkc test` observes storage as workerd does. Each later slice added its runtime proof
    there. The baseline fixture's expected failures were deleted one slice at a time, reaching
    `passed=10 failed=0` at #1690.
  - **G1** the doc gate checks every `bynk` block ([#1661](https://github.com/accuser/bynk/issues/1661)),
    and **G2** brought diagnostic coverage and registry drift under the gated `diagnostic_coverage`
    probe, at a floor of 4 ([#1662](https://github.com/accuser/bynk/issues/1662)). Both shipped in
    [#1701](https://github.com/accuser/bynk/pull/1701), v0.299.1.
  - **G3** diagnostic recovery ([#1663](https://github.com/accuser/bynk/issues/1663), ADR 0433,
    [#1709](https://github.com/accuser/bynk/pull/1709), v0.302.0):
    - per-declaration resolve-then-check;
    - a skipped declaration's name still known;
    - error-typed bindings for failed patterns and `let`s;
    - every syntax error reported by the CLI;
    - an exact-set mode for negative fixtures, so "no spurious diagnostics" is now checkable.

  **Done-when, met.**
  - Every §4 slice landed.
  - The four §5 ADRs are written (0421, 0422, 0423, 0425).
  - `behaviour_fixtures.rs` runs in CI under `BYNK_REQUIRE_TSC`.
  - `InMemoryStorage` clones.

  **Follow-ons, named rather than left implicit; all stay open:**
  - [#1697](https://github.com/accuser/bynk/issues/1697): emitted names derived from user names (S4);
  - [#1691](https://github.com/accuser/bynk/issues/1691): held-map coverage (S14);
  - [#1686](https://github.com/accuser/bynk/issues/1686): leaked workerd processes;
  - [#1703](https://github.com/accuser/bynk/issues/1703): a suite whose target uses a generic type fails
    `tsc`;
  - [#1706](https://github.com/accuser/bynk/issues/1706): a stub `fails` throws where the docs promise
    an `Err`. This one needs a language decision;
  - [#1708](https://github.com/accuser/bynk/issues/1708): `bynkc check` panics on a suite property body
    with a type error, found by G3's mutation sweep;
  - [#1710](https://github.com/accuser/bynk/issues/1710): partial-parse checking on the project path,
    which G3 named as out of scope.

  The 2026-10-01 review's other track, toolchain pins as gates
  ([#1670](https://github.com/accuser/bynk/issues/1670)), is separate and unaffected. Retired 4 October 2026.

- **Toolchain pins as gates** — spine [#1670](https://github.com/accuser/bynk/issues/1670), opened
  from the [2026-10-01 language-implementation review](../reviews/2026-10-01-language-implementation-review.md)
  ([#1647](https://github.com/accuser/bynk/pull/1647)). Part 0 of that review re-measured the eight asks
  of the [3 September toolchain review](../reviews/2026-09-03-compiler-toolchain-review.md) and found that
  none had landed or been filed. The Rust half of the toolchain was pinned with care; the Node half (the
  tools that run emitted code) stated versions to users as if they were tested contracts. The track's
  theme was the review's own: "the fix in every case is a gate, not a bump". Its end state was every
  external-tool version the toolchain states to users either checked by a gate or covered by a written
  policy, and the release running the same suite, the same way, as the PR gate.

  The track never had a doc in `design/tracks/`. Its open questions were small enough that each slice's
  issue carried its own decisions, and the spine's checklist served as the track. So this retirement
  removes no doc and changes no row in the tracks table.

  **What shipped** (all seven slices, 5 October 2026):
  - **T1 + T4** ([#1671](https://github.com/accuser/bynk/issues/1671),
    [#1674](https://github.com/accuser/bynk/issues/1674);
    [#1713](https://github.com/accuser/bynk/pull/1713), v0.303.1):
    - `NODE_MAJOR_FLOOR` raised from 18 to 22, the oldest supported Node;
    - the CI and release test legs on Node 22, so the strip-types gate runs for real;
    - its skips panic under `BYNK_REQUIRE_TSC`.
  - **T2** ([#1672](https://github.com/accuser/bynk/issues/1672),
    [#1714](https://github.com/accuser/bynk/pull/1714), v0.303.2):
    - emitted TypeScript verified under TypeScript 5 (the floor) and 7 (the current major);
    - `bynk doctor` checks the `tsc` major, warning below the floor and reporting an untested newer major
      without failing `--strict`;
    - every `npx` fallback aligned on one constant.
  - **T5** ([#1675](https://github.com/accuser/bynk/issues/1675),
    [#1727](https://github.com/accuser/bynk/pull/1727), v0.303.3,
    [ADR 0434](../decisions/0434-act-on-compiler-skew.md)):
    - wherever `bynk` runs a second compiler, it acts on `doctor`'s skew classification;
    - that is `bynk test` always, and `check`, `fmt`, `dev` and `deploy` under `BYNK_BYNKC`;
    - a minor skew warns once per process, and a major skew refuses unless allowed.
  - **T6** ([#1676](https://github.com/accuser/bynk/issues/1676),
    [#1726](https://github.com/accuser/bynk/pull/1726)):
    - the release and bootstrap gates run `cargo nextest run --workspace --locked --profile ci`, like the
      PR gate;
    - the retry policy is stated once, in `.config/nextest.toml`;
    - drift guard: `xtask/tests/suite_harness.rs`.
  - **T7** ([#1677](https://github.com/accuser/bynk/issues/1677),
    [#1728](https://github.com/accuser/bynk/pull/1728)):
    - every `wasm-bindgen-cli` install must be pinned to `Cargo.lock`'s `wasm-bindgen` version
      (`xtask/tests/wasm_bindgen_pin.rs`);
    - the stale 0.2.126 pin moved to 0.2.127;
    - the Workers `compatibility_date` policy is Part 3 of
      [`../bynk-release-discipline.md`](../bynk-release-discipline.md): reviewed at each release, moved
      only after the workerd smokes pass. The date itself stayed at `2024-11-01`.
  - **T3** ([#1673](https://github.com/accuser/bynk/issues/1673),
    [#1729](https://github.com/accuser/bynk/pull/1729), v0.303.4):
    - the VS Code extension's `bynkServerVersion` names the last shipped release, not the workspace
      version. It had named releases that didn't exist since `v0.290.0`;
    - `release.yml`'s `server-pin` job moves it after each release;
    - `scripts/check-server-pin.sh` checks the release on every CI run and in `verify`;
    - the extension judges a `PATH` server by its own version.

    Its Decision B, the release cadence, was the maintainer's call. Monthly milestones are the cadence
    from 1.0. **Before 1.0, a release is tagged at each track retirement** (step 6 of the lifecycle in
    [`../tracks/README.md`](../tracks/README.md)), plus any user-facing fix that can't wait. This
    retirement is the first release under that rule.

  **Done-when, met.** Each version the toolchain states to users is now gated or written down:
  - TypeScript: by the 5/7 verification and `doctor`;
  - Node: by the floor and the Node 22 legs;
  - the LSP server release: by `check-server-pin.sh` and the offline `server_pin.rs`;
  - `wasm-bindgen-cli`: by its drift test;
  - the compatibility date: by a written policy.

  The release runs the PR gate's harness and profile.

  **Not duplicated:** the review's ask 8, repointing the dead `design/tracks/*.md` citations, is slice S8
  of the crate-hygiene track [#1533](https://github.com/accuser/bynk/issues/1533) and stays there.

  **Follow-ons, named rather than left implicit:**
  - [#1686](https://github.com/accuser/bynk/issues/1686): leaked workerd processes. This stays open and
    affects local runs of the gates this track tightened.
  - On 0.x every release is a minor skew, so ADR 0434's major refusal can't trigger before 1.0. This is
    recorded in the ADR. Treating 0.x minor skew as major would change `Skew::classify`, and with it what
    `doctor` reports, so it is left as a question for 1.0 rather than a filed issue.

  Retired 5 October 2026.
- **`interop.md`** — spine [#1973](https://github.com/bynk-lang/bynk/issues/1973): adopting Bynk one
  context at a time beside existing TypeScript Workers, in both directions. The cross-context wire
  format becomes a published contract, a caller that is not Bynk gets an identity a handler can tell
  apart from a Bynk context, that caller builds against a generated typed client, and a Bynk context
  can call a Worker that is not Bynk over a Service Binding. Settled 16 October 2026 (settling PR
  [#1996](https://github.com/bynk-lang/bynk/pull/1996)), with all nine settling questions resolved
  with the maintainer that day.

  **The settling survey found a live defect before any slice ran:** `/_bynk/call/` was
  reachable on every route a Worker served, so `X-Bynk-Caller` could be forged from the public
  internet, not only by a Worker holding a Service Binding. That contradicted ADR 0092's premise. It
  was fixed first, outside the track (Q1).

  **What shipped**, in order (all 16 October 2026):
  - **I0, out of track:** a per-deployment call secret, `BYNK_CALL_SECRET`, sent as
    `X-Bynk-Call-Secret` and checked on every `/_bynk/call/` and `/_bynk/event/` request before
    anything is decoded ([#2004](https://github.com/bynk-lang/bynk/pull/2004), v0.335.0,
    [ADR 0479](../decisions/0479-deployment-call-secret.md)).
  - **I5 (D4, #1972):** experimental `bynk.cloudflare` `Service`,
    `capability Billing = Service @worker("legacy-billing")`, a derived capability bound as
    `SERVICE_BILLING`. `bynk deploy` warns when the named Worker is missing, and `bynk dev` binds a
    stand-in ([#2008](https://github.com/bynk-lang/bynk/pull/2008), v0.335.1,
    [ADR 0480](../decisions/0480-foreign-worker-service-capability.md)).
  - **I1 (D1, #1969):** wire format version 1, generated from the compiler's `WireModel` with a drift
    guard, published as the [wire format reference](../../site/src/content/docs/book/reference/wire-format.md); callers send
    `X-Bynk-Wire: 1` ([#2009](https://github.com/bynk-lang/bynk/pull/2009), v0.338.0,
    [ADR 0483](../decisions/0483-wire-format-is-a-published-surface.md)).
  - **I2 (D2 mechanism, #1971):** every cross-context call and event delivery is signed with
    HMAC-SHA256 under a key one caller shares with one callee (`BYNK_CALL_TO_<CALLEE>` /
    `BYNK_CALL_FROM_<CALLER>`), replacing I0's shared secret
    ([#2012](https://github.com/bynk-lang/bynk/pull/2012), v0.340.0,
    [ADR 0485](../decisions/0485-per-edge-call-signature.md)).
  - **I3 (D2 surface, #1971):** a context admits callers that are not Bynk by declaring each as an
    actor with the `Foreign` scheme, `actor Ledger { auth = Foreign(secret = "…", worker = "…") }`;
    `bynk deploy` mints the edge key and prints the `wrangler secret put` hand-off
    ([#2015](https://github.com/bynk-lang/bynk/pull/2015), v0.341.0,
    [ADR 0486](../decisions/0486-foreign-caller-actors.md)).
  - **I4 (D3, #1970):** a context that declares a foreign caller gets a typed TypeScript client,
    `workers/<worker>/bynk-client.ts`, typed per caller so an unadmitted call fails `tsc`, with its
    runtime in `@bynk/wire-client` (`bynk-emit/wire-client`) and foreign callers pinned in
    `bynk.foreign.toml`, whose breaking hash change `bynk deploy` refuses
    ([#2023](https://github.com/bynk-lang/bynk/pull/2023), v0.343.0; no ADR: the track's ADR plan assigned none to I4, whose decisions D3/Q6–Q7 are recorded below).

  **The settled decisions**, for the code comments that cite them:
  - **D1 / Q2:** the wire format is versioned independently of the compiler, like ADR 0310's ABI;
    the callee accepts a range of `X-Bynk-Wire` versions, and the contract hash still detects
    signature skew within a version (ADR 0483).
  - **D2 / Q3–Q5:** a `Foreign` actor scheme, with `CallerId` staying a `String`; the deploy mints
    edge secrets and prints the hand-off; every internal request is signed, binder-less `on call`
    handlers included (ADRs 0485, 0486).
  - **D3 / Q6–Q7:** the callee's deploy *refuses* a hash change that breaks a declared foreign
    caller's pin (updating the pin is the override); the client runtime is a separate package
    versioned with the wire format, not part of ADR 0310's emit ABI.
  - **D4 / Q8:** deploy warns, never refuses, on a missing named Worker; dev binds a per-instance
    stand-in (ADR 0480).
  - **Q9:** native `WorkerEntrypoint` RPC is recorded as a candidate for a future track, not built.

  **Follow-ons, named rather than left implicit:**
  - Native `WorkerEntrypoint` RPC (Q9): a candidate for a future track, worth doing if Bynk-to-Bynk
    calls move to RPC too.
  - `@bynk/wire-client` is not published to npm; the docs say to depend on it from a file path or
    git. The generated client does not compile under `noUncheckedIndexedAccess`, documented as a
    limitation.
  - [ADR 0490](../decisions/0490-contract-mismatch-body-keeps-expected.md) (#2054) keeps `expected`
    in the `409 ContractMismatch` body, since only an authenticated caller can reach it and removing
    it would need wire format version 2.

  This track's doc, `design/tracks/interop.md`, is removed. Code comments that cited its sections now
  cite this entry. Retired 17 October 2026.

- **`documentation.md`** — spine [#557](https://github.com/bynk-lang/bynk/issues/557): documentation and web
  presence, from the mdBook book to a single site at `bynk-lang.org`. It pre-dates the GitHub-native flow,
  so its doc was committed by an ordinary PR and its slices were not filed as sub-issues. They shipped as
  ordinary PRs:
  - **Slice 0 (#401):** the playground deploys to Cloudflare Pages, with the app and its cross-origin
    `sandbox.` execution document as two origins (`deploy-playground.yml`, ADR 0140).
  - **Slice 1 (#404):** the Astro + Starlight shell on the apex, Shiki fed the editor's own
    `bynk.tmLanguage.json`, Pagefind search, and an internal link-check gate
    ([ADR 0141](../decisions/0141-documentation-framework.md)).
  - **Slice 2 (#406, #408, #410):** the Book ported to Starlight, with the grammar and diagnostics embeds
    reimplemented as build steps and the mdBook generators and gates rewired to `site/`.
  - **Slice 3:** the verification harness is `bynkc/tests/doc_examples.rs`, which compiles every fenced
    `bynk` block in the Book, developer docs and landing page, with `fail`, `fail=<code>`, `fragment` and
    `ignore` forms ([#1661](https://github.com/bynk-lang/bynk/issues/1661),
    [#1701](https://github.com/bynk-lang/bynk/pull/1701)).
  - **Slice 4 (#417, #418):** Bynk by Example, with project pages from `examples/` and a snippets tier,
    each runnable block carrying a build-time playground deep link.
  - **Slice 5 (#420, #424):** the developer docs (CLI, manifest, emission, tooling) re-homed under `/docs/`,
    plus generated crate API docs at `/docs/api/`.
  - **Slice 6 (#415, #499):** the real landing page and first brand pass; the placeholder `noindex` was lifted.
  - **Slice 7 (#428, #430):** inline playground embeds (`<Example runnable embed />`), used by the first tutorial.

  **The settled decisions**, for the code comments that cite them:
  - **Framework:** Astro + Starlight, unifying the Book, Bynk by Example and the developer docs (ADR 0141).
  - **Highlighting:** one grammar, `bynk.tmLanguage.json`, for the editor and the site; the playground's
    wasm tree-sitter highlighter stays the exact fallback if it proves lossy.
  - **URLs:** transient before 1.0, with a single "latest" and no legacy redirects.
  - **Repository identity:** the repository and crate metadata are unchanged; only the docs moved.
  - **Search:** Pagefind, local and static.

  **Follow-ons, named rather than left implicit** (unfiled):
  - A **grammar-agreement check** between the tmLanguage and tree-sitter (§5.1's highlighting corpus and
    node-type → scope mapping). Only `bynkc/tests/textmate_keywords.rs` shipped: every reserved keyword
    appears in the grammar, which is weaker than the mapping the doc specified.
  - A **deep-link round-trip conformance test**. `site/src/lib/examples.mjs` is a port of
    `playground/src/deeplink.ts`, and nothing yet asserts the two encodings stay in step.
  - The **manifest page's final shape**, which waits on the packaging track
    ([#843](https://github.com/bynk-lang/bynk/issues/843)); until then it documents today's `bynk.toml`.
  - Decisions the doc deferred to 1.0 or on demand: versioned docs and permalink stability, analytics,
    and a feedback affordance.

  This track's doc, `design/tracks/documentation.md`, is removed. Code comments that cited its sections now
  cite this entry. Retired 19 October 2026.

- **`end-to-end-testing.md`** — spine [#2133](https://github.com/bynk-lang/bynk/issues/2133): a
  project's whole-system scenarios as `system` cases, written in Bynk, type-checked against the
  routes they call, run in process under `bynk test`, and run unchanged against `bynk dev` or a
  deployed preview with one runner flag. It extends the testing track's "tier is a dial" pillar
  ([ADR 0153](../decisions/0153-tier-is-a-dial-on-the-case-header.md)) to the rung ADR 0207 had left
  outside the language, and takes up the follow-ons the `system`-tier ADRs named (queue at `system`,
  a `Signature` signer). The worked example was `bynk-lang/shop`'s `e2e/scenarios.sh`, about 200
  lines of Bash run against `bynk dev`. Settled 18 October 2026 (settling PR
  [#2140](https://github.com/bynk-lang/bynk/pull/2140)), with all nine questions (§3.0–§3.8) settled
  in the direction the draft leaned.

  **What shipped** (v0.353.4–v0.371.0):
  - **Slice 0, honesty fixes:** check and emit made to agree on what `system` couldn't do yet. A
    cron or queue address in a `system` case was refused instead of emitting `/* unknown */`
    ([#2131](https://github.com/bynk-lang/bynk/issues/2131), #2141), and so was a call to a
    `Signature`-secured route, which could only fail
    ([#2132](https://github.com/bynk-lang/bynk/issues/2132), #2143). Two follow-ons closed the same
    class of gap: a call to an `Oidc`-secured route, which could only get a `401`, was refused
    ([#2150](https://github.com/bynk-lang/bynk/issues/2150), #2151), and an actor-sum route's
    credential is now picked per call from the call-site principal rather than the primary member,
    which retired #2132's last refusal
    ([#2155](https://github.com/bynk-lang/bynk/issues/2155), #2160, v0.368.0,
    [ADR 0529](../decisions/0529-system-tier-sum-route-credential.md)).
  - **Slice 1, suite configuration (#2134):** `env { NAME = "value" … }` in a suite or case body,
    one map for every participant, legal at every tier
    ([#2153](https://github.com/bynk-lang/bynk/pull/2153), v0.364.0,
    [ADR 0524](../decisions/0524-suite-env-configuration.md)).
  - **Slice 2, a `Signature` signer (#2135):** the `system` driver signs every call to a
    `Signature` route as the actor declares, with reserved `Forged` and `Stale` principals for the
    failures ([#2154](https://github.com/bynk-lang/bynk/pull/2154), v0.365.0,
    [ADR 0525](../decisions/0525-system-tier-signature-signer.md)).
  - **Slice 3, `cron` and `queue` at `system` (#2136):** driven through the Worker's real
    `scheduled` and `queue` entries, yielding the handler's own result
    ([#2152](https://github.com/bynk-lang/bynk/pull/2152), v0.362.0,
    [ADR 0522](../decisions/0522-cron-and-queue-at-the-system-tier.md)).
  - **Slice 4, the harness clock (#2137):** one clock per case from the epoch, moved only by
    `advance Clock by <Duration>`, at every tier
    ([#2156](https://github.com/bynk-lang/bynk/pull/2156), v0.363.0,
    [ADR 0523](../decisions/0523-the-test-harness-owns-the-clock.md)).
  - **Slice 5, run against a real runtime (#2138),** in five PRs:
    - `bynk dev --ready-json <file>`, written once every context answers a probe
      ([#2158](https://github.com/bynk-lang/bynk/pull/2158), v0.366.0,
      [ADR 0526](../decisions/0526-bynk-dev-readiness-file.md));
    - `bynk test --against <name>` for a target under `[test.targets]`, with the remote refusals
      ([#2159](https://github.com/bynk-lang/bynk/pull/2159), v0.367.0,
      [ADR 0527](../decisions/0527-end-to-end-testing-is-the-languages.md), which supersedes ADR
      0207's "e2e is external" line, and
      [ADR 0528](../decisions/0528-test-against-a-named-target.md));
    - `bynk test --against dev`, one local `bynk dev` per distinct `env`
      ([#2161](https://github.com/bynk-lang/bynk/pull/2161), v0.369.0,
      [ADR 0530](../decisions/0530-test-against-a-local-bynk-dev.md));
    - `--settle`, retrying a failing remote `expect` by a static replay rule
      ([#2164](https://github.com/bynk-lang/bynk/pull/2164), v0.370.0,
      [ADR 0531](../decisions/0531-settle-retries-against-a-real-runtime.md));
    - `run.id`, the `bynk.test.remote_clock_dependent` warning, and the guide
      [*Testing against a running system*](../../site/src/content/docs/book/guides/testing/against-a-running-system.md)
      ([#2165](https://github.com/bynk-lang/bynk/pull/2165), v0.371.0,
      [ADR 0532](../decisions/0532-test-run-identity.md),
      [ADR 0533](../decisions/0533-remote-clock-dependent-warning.md)).

  **The settled decisions**, for the code comments that cite them:
  - **§3.0:** end-to-end testing is the language's. The non-determinism of a remote run is
    contained, not avoided: the run is opt-in (`--against`), retries are reported per case, and the
    in-process `system` tier stays the default and the gate. ADR 0431's determinism holds for every
    run without `--against` (ADR 0527).
  - **§3.1:** test configuration lives in source as `env { … }`, one map for every participant, a
    case's map layered over its suite's, `String` values only. At `system` the entries go into each
    participant's `env`; at `unit`/`integration` they are what the `Secrets` double answers. An entry
    naming an actor's secret replaces the fixed test secret for signing and verification. No
    `[test.env]` manifest table (ADR 0524).
  - **§3.2:** plain calls to a `Signature` route are signed with the actor's declared `secret`,
    `header` and `timestamp`, over exactly the bytes sent. `by Nobody` sends no signature, `by Forged`
    signs with a wrong secret, and `by Stale` signs `tolerance + 60` seconds in the past; `Stale` is
    refused where the actor declares no `timestamp` or no `tolerance` (ADR 0525). On an actor sum the
    credential follows the call-site principal (ADR 0529).
  - **§3.3:** `queue` is admitted to `system` as a serialisation edge; `cron` is legal only in a
    suite that already qualifies. The address yields the handler's own result (`Ack`/`Retry` for a
    queue, the stored `Result` for cron), a throwing handler maps as the platform would, and the
    scheduled time stays a required argument (ADR 0522).
  - **§3.4:** the harness owns the `Clock` capability, and only that, at every tier: it starts at
    the epoch and moves only by `advance Clock by`, the principled exception to
    `bynk.stub.at_system`. The runtime's own seams keep wall-clock time, and the deploy output is
    unchanged. It does not promote to a remote run: `advance Clock` is refused remotely, and
    `bynk.test.remote_clock_dependent` warns on an `expect` comparing instants that are not all
    run-derived (ADRs 0523, 0533).
  - **§3.5:** where the Workers run is a property of the run, not the case: no `as e2e` tier.
    `--against dev` or `--against <name>`; `bynk dev --ready-json` for readiness; one transport
    function in the drivers (ADRs 0526, 0528, 0530).
  - **§3.6:** no new syntax. Under `--against`, a failing `expect` is retried up to `--settle`
    (default `5s` remotely, `0` in process) by re-running the addresses since the last unsafe one,
    marked at compile time; retries are reported per case (ADR 0531).
  - **§3.7:** when a remote run is requested the checker refuses (`bynk.test.remote_unsupported_*`)
    a case that reads an agent, addresses an `on call` service, or uses `advance Clock` or
    `svc.event(…)`; against a named target, it also refuses a case whose non-secret `env` disagrees
    with the target's `vars`, or that names a secret. `--skip-unsupported` skips such cases instead,
    naming each one. `run.id` is fixed in process and fresh per remote run (ADRs 0528,
    0532).
  - **§3.8:** `--against` takes only `dev` or a name under `[test.targets]`, never a bare URL; a
    `production = true` target is refused; a named target's secrets come only from the environment;
    the runner prints the target and each Worker's URL before the first request (ADR 0528).

  **Follow-ons, named rather than left implicit:**
  - **Porting `bynk-lang/shop`'s scenarios.** The track doc's acceptance criterion (§4, §8) was that
    the shop's `e2e/scenarios.sh` and the `bynk-dev` step in its `e2e.yml` are deleted, with its
    scenarios as `system` cases run in process and with `--against dev`. At retirement the shop had
    not been ported. The maintainer ruled this downstream adoption in `bynk-lang/shop`, not track
    scope: everything it needs from the language has shipped.
  - [#2175](https://github.com/bynk-lang/bynk/issues/2175): review polish on `run.id` and the
    remote clock warning, from #2165's review.
  - Decisions the settling pass left re-openable: per-context `env` maps (§3.1), on the first
    project that needs two contexts to see different values under one name; a `[test.env]`
    project-wide default (§3.1), as a re-settle; an explicit `by Signed(secret = …, at = …)` (§3.2),
    if a test needs one specific timestamp.

  This track's doc, `design/tracks/end-to-end-testing.md`, is removed. Code comments that cite its
  sections now resolve to this entry. Retired 19 October 2026.

- **`agent-state-views.md`** — spine [#2190](https://github.com/bynk-lang/bynk/issues/2190): what an
  agent's `on call` handlers do to its state, not who they talk to. The sequence diagram drew an
  agent handler as a box of branches with none of its writes in it; the Book's state-machine guide
  drew the `Order` lifecycle by hand. The track made that view mechanical: a state model derived from
  the handlers, rendered live in the editor, with `transition` step invariants
  ([ADR 0107](../decisions/0107-agent-invariant-predicate-surface.md)) checked against it. Editor
  tooling, no language surface. Settled 19 October 2026 (settling PR
  [#2191](https://github.com/bynk-lang/bynk/pull/2191)), with all seven questions (§3.1–§3.7) settled
  in the direction the draft leaned.

  **What shipped** (v0.371.6–v0.372.2):
  - **Slice 0, state writes in the sequence diagram**
    ([#2194](https://github.com/bynk-lang/bynk/issues/2194), #2200, v0.371.6,
    [ADR 0534](../decisions/0534-sequence-diagram-state-writes-are-notes.md)): each `:=` and mutating
    store operation in an agent handler is a note on the entry lifeline, in branch order, linked to
    the write. The store-op registry gained a `mutates` flag, now the single source for this
    recogniser and the emitter's commit detection.
  - **Slice 1, the agent state model**
    ([#2195](https://github.com/bynk-lang/bynk/issues/2195), #2202, v0.371.7,
    [ADR 0535](../decisions/0535-agent-state-model-data-source.md)): `bynk_ide::agent_state`, a pure
    function of an `AgentDecl` and the type table, walks each handler's paths to edges
    (`from` set × handler × guards → target, outcome) per state field, with reachability from the
    initialiser and a 256-path budget.
  - **Slice 2, the state view**
    ([#2196](https://github.com/bynk-lang/bynk/issues/2196), #2206, v0.371.8,
    [ADR 0536](../decisions/0536-agent-state-view.md)): the `bynk/agentStateModel` request, a
    `Show State` lens on each agent, and an agent-scoped live panel with a Mermaid state diagram and
    the handler × state table, both linking to source, highlighting the handler under the cursor.
  - **Slice 3, the `transition` overlay**
    ([#2197](https://github.com/bynk-lang/bynk/issues/2197), #2208, v0.372.1,
    [ADR 0539](../decisions/0539-transition-overlay-is-a-view-annotation.md)): edges a `transition`
    refuses from a reachable state are flagged, known or possible per start state, in handlers that
    commit; a `transition` outside the evaluated fragment is listed as not evaluated.
  - **Slice 4, the per-handler effect summary**
    ([#2198](https://github.com/bynk-lang/bynk/issues/2198), #2212, v0.372.2,
    [ADR 0540](../decisions/0540-agent-effect-summary.md)): each handler's writes, reads, agent calls
    and outcomes per merged path, in place of the empty state for an agent with no state field and
    collapsed under the machine otherwise.

  **The settled decisions**, for the code comments that cite them:
  - **§3.1:** a state field is a `store` `Cell` of a user enum or sum type, each constructor a state
    with payloads abstracted; `Option`, `Result`, records and scalars are not. One diagram per state
    field, chosen with a selector. An agent with none gets the effect summary.
  - **§3.2:** the view is agent-scoped, in its own live panel (ADR 0504), opened by a `Show State`
    lens on the agent or the editor title bar. It retargets when the enclosing agent changes and only
    moves the highlight between handlers. It does not share the sequence diagram's panel.
  - **§3.3:** the model is a re-parse of the committed snapshots, not retained checker callees, so
    `ProjectAnalysis` is unchanged (the analogue of ADR 0301). Writes are recognised syntactically,
    respecting shadowing. Paths are enumerated over the sequence diagram's statement-level scope
    (ADR 0259), merged at joins when they produce the same edge, and bounded by a path budget; a
    computed write (`f.update(…)`, `f := g(x)`) goes to `?`, which counts as reaching every state.
    Reads after a write on the same path see the written value.
  - **§3.4:** both a diagram and a table, with the table the exact record: total over handler ×
    state, each cell listing every path with its guard and outcome. The diagram draws state changes
    and `[*] →` the initialiser; unchanged outcomes are self-loops behind a toggle. Click-to-code maps
    the rendered SVG back to the model, because Mermaid's `click` in `stateDiagram-v2` takes a URL
    only.
  - **§3.5:** a statically violable `transition` is a view annotation, not a diagnostic. The overlay
    evaluates a closed fragment (`old.f`/`new.f` `is`/`==` a variant, under `!`, `&&`, `||`,
    `implies`); anything else is not evaluated, never guessed. Unchanged paths count as `S → S` only
    in a handler that commits, and only reachable starts are flagged. A flagged edge is never hidden.
  - **§3.6:** state writes are notes on the entry lifeline, not a lifeline: an agent's store is its
    own storage, not a runtime boundary (ADR 0258 unchanged). Agent handlers only.
  - **§3.7:** the edge record is the natural denotation of a future declared transition table, so
    the extractor stays a dependency-free function of `AgentDecl` and the type table that a checker
    rule could reuse from `bynk-check`. Nothing is built or promised about table syntax.

  **Follow-ons:** a checker diagnostic for a statically violable `transition` (§3.5 keeps it a
  separate decision), and the declared transition table language increment the agents reference
  defers (§3.7), which would move the extractor below `bynk-ide`.

  This track's doc, `design/tracks/agent-state-views.md`, is removed. Code comments that cited its
  sections now cite this entry. Retired 20 October 2026.
