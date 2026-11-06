# 0740 — OpenAPI, the smoke runner and entry signatures move onto the system model, with protocols left where the consumer doesn't hold the model's input

- **Status:** Accepted (v0.510.21)

**Context.** ADR 0735 made `bynk-model` the system model and set M-1 to M-4,
including M-1's domain clause: "M-1 applies where the consumer holds the model's
input". B1 (#2939) moved the editor onto the model's handler kinds.

Two consumers still re-derive handler kinds and HTTP methods from the AST:
- `bynk openapi` (`bynk/src/openapi.rs`): 2 `HandlerKind::` and 2 `ServiceProtocol::` lines.
- The smoke runner (`bynk-driver/src/smoke.rs`): 2 `HandlerKind::`, 1 `ServiceProtocol::` and 1
  `HttpMethod::` line.

The playground's entry signatures (#2695, `bynk_check::entry_signature`) are a
declaration-level description that lives outside the model. Slices B2 and B3
(#2999) take these on. Counts are from `bb08a1835`, with the gated probes' rule.

**Decision.**

- **A. openapi's protocol filters stay.** `document` and `declared_statuses`
  answer over a certified project, but through `ProjectDiagnostics`, which carries
  no `TypedCommons`. `lower_protocol_ir(_from_commons)` needs one. By the clause's
  first sentence, M-1 doesn't apply to those two lines, and each cites the clause.
  The gap is a named input to B4, which decides where per-service boundary facts
  live beside `WireModel` and `ArchModel`. Its handler-kind matches move onto
  `lower_handler_kind_ir`, which needs only the AST.
- **B. The smoke runner's protocol filter stays.** `Smoke::collect` reads a
  recovering parse before `settle` checks the project, so it holds no commons. Its
  handler-kind and method sites (and `Route.method`) move onto
  `IrHandlerKind`/`IrHttpMethod`. `IrHttpMethod::as_str` spells every method as
  `HttpMethod::as_str` does, so the generated suites can't change.
- **C. Entry signatures move verbatim** into `bynk_model::entry`. Their AST reads
  become the builder's, which is where M-1 puts AST reading. Rebuilding
  `EntryShape` over `TypeShape` is a B4 input: `lower_type_shape_ir` needs a
  `CheckedProgram` that `build_output` no longer holds, and `TypeShape::Refined`
  embeds the AST's `BaseType`. `display_or_unknown`'s unknown branch is a display
  string for a type a host can't build, not a model fact, so it is a documented M-2
  exception.
- **D. The test-module emitter's AST matches are out of scope.** These are in
  `bynk-driver/src/project/tests_emit`, which D1 moved out of `bynk-emit`.
  They're tracked in #3000.
- **E. `MAX_DEPTH` becomes `pub(crate)`** before the move, since nothing outside
  its module reads it. Moved as `pub`, it would fail `unconsumed_ir_items`.
- **F. Direct edges, no facades.** `bynk` and `bynk-wasm` each depend on
  `bynk-model` directly. Neither `bynk-ide` nor `bynk-driver` re-exports the
  builder. `bynk_check::entry_signature` is removed without a re-export, because
  `bynk-check` sits below the model.

**Rejected.**
- A commons-free "protocol kind" lowering in the model. It is a second enum for one
  fact, the mirror shape B1's criteria ruled out.
- Keeping every unit's `TypedCommons` in the editor's analysis so openapi can lower
  protocols. Every editor analysis would retain every file's typed commons to serve
  two lines of one CLI command.
- Collecting smoke routes after a certified check. It needs a second full project
  check per run, and changes when a project error is reported.

**Consequences.**
- B2 is probe-invariant. `model_readers_beyond_emit` lists items, and the three items
  B2's sites name (`IrHandlerKind`, `IrHttpMethod`, `lower_handler_kind_ir`) are
  already on it.
- B3's move raises the reading from 3 to 7. Two of the four new names are text
  collisions in `bynk-wasm`: its own `EntryParam` struct, and the `entry_handlers`
  field.
- The remaining `ServiceProtocol::` matches outside the model (the editor's 8,
  openapi's 2, the smoke runner's 1) each cite M-1's domain clause.
- The smoke suites (`bynk-driver/tests/snapshots/smoke_suites.txt`), the openapi
  goldens, #2944's handler-kind snapshot and B3's entry pin hold the output
  byte-identical (H-1).
