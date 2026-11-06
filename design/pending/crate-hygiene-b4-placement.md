---
level: patch
changelog: The system model's placement is settled. It stays above `bynk-check`, the wire contract stays in `bynk-check` as the model's boundary half, and the editor's architecture map stays an editor view. Whether the model becomes a public interface is deferred past 1.0 (crate-hygiene B4)
---

## ADR: system-model-b4
title: The system model stays above the checker, with the wire contract and the architecture map left where they are
summary: Slice B4 of the crate-hygiene track settles where the system model sits, where WireModel and ArchModel live, which AST matches stay under M-1's domain clause for good, and defers EntryShape-over-TypeShape and the public interface past 1.0

**Context.** ADR 0735 placed `bynk-model` above `bynk-check` for slices B1–B3 and
left two questions to B4:
- whether the model moves below the checker (track §6.3 Q1(b));
- whether it becomes a public interface (Q4, "deferred until B3").

ADR 0740 named three inputs to B4:
- openapi's protocol filters, because `ProjectDiagnostics` carries no `TypedCommons`;
- rebuilding `EntryShape` over `TypeShape`;
- the driver's test-module emitter's AST matches (#3000).

B4 (#3012) measured each at `1830e38c6`. This ADR amends ADR 0735 (Q1, Q4) and ADR 0740 (its
three B4 inputs).

**Decision.**

- **A. The model stays above `bynk-check` (Q1(a)), settled for good.** Option (b) would
  need four things:
  - moving `TyId`, the `Types` arena and `Ty` out of `checker.rs` (54 files outside
    `bynk-check` name `TyId`);
  - moving or renaming the `actors` seam types and `Callee`;
  - doing S7;
  - splitting the builder back out of `bynk-model`, which reverses ADR 0735's merge.

  It would buy one home for `WireModel`, which already meets M-1. The track's §10
  wire/contract split is re-pointed to the checker-core split, since Q1(b) never fires it.
- **B. `WireModel` stays in `bynk-check`**, documented as the model's boundary half. Its
  production code reads only the AST and type table, and the checker reads it at six sites.
- **C. `ArchModel` stays in `bynk-ide`, as an editor view.** It is built over a recovering
  parse of unsaved editor text, so it can't meet M-2 ("built after `certify`"). Its
  constructor would be a second one beside the builder. Six of its names have no reader
  outside `bynk-ide` by name. Its AST matches fall under M-1's domain clause. A fact the
  model carries is read from the model.
- **D. openapi's two `ServiceProtocol::Http` filters stay under the domain clause for good.**
  Carrying a per-service `ProtocolIr` map on `ProjectDiagnostics` would add a field every
  editor round computes, read by one CLI command, to remove two lines whose fact involves no
  type. *Trigger to revisit:* a second reader of per-service protocol facts there.
- **E. The smoke runner's protocol filter stays under the domain clause for good.** It answers
  over a recovering parse, which is the clause's own case. ADR 0740 already rejected the only
  fix.
- **F. Rebuilding `EntryShape` over `TypeShape` is deferred past 1.0.** `build_output` holds
  no `CheckedProgram`, `TypedCommons` or `Types` arena. Both derivations are the model's own
  builder, so this is duplication inside the model, not an M-1 defect. *Trigger:* a
  type-shape change that has to be made in both, or the model becoming public.
- **G. The test-module emitter (#3000) reads handler kinds through
  `lower_handler_kind_ir`.** Its protocol matches cite the domain clause, because none of
  `emit_system_http_support`, `emit_system_job_support` and `event_delivery_helpers` holds
  the target's `TypedCommons`.
- **H. The public interface (Q4) is deferred past 1.0.** Publishing the model's shape before
  then would freeze a contract while the model is still moving. `bynk-lsp`'s
  `WireArchModel` and MCP's renderers serialise views, not the model. *Trigger:* a tool
  outside this repository asks for one description of a project. Model items stay plain
  data, with no lifetimes and no references into the AST.

**Rejected.**
- Moving the model below the checker (Q1(b)), for the cost in A.
- A new crate below the checker for `WireModel` alone. It is a crate for a placement
  preference, against the track's H-2 and its pre-1.0 aim of fewer crates.
- Moving `ArchModel`'s shapes and builder into the model with an M-2 exception.
- Building `synthetic_typed_commons_for_target` in the test-module emitter to lower
  protocols. It rebuilds a commons from the symbol tables to answer what the AST already
  answers.

**Consequences.**
- B4 is probe-invariant. `model_readers_beyond_emit` stays at 7 and `unconsumed_ir_items` at
  0. #3000's sites are in `bynk-driver`'s `project` module, which the probe excludes, and
  every item they name is already read.
- The `ServiceProtocol::` matches outside the model are now permanent, each citing the domain
  clause:
  - the editor's 8;
  - openapi's 2;
  - the smoke runner's 1;
  - the test-module emitter's 4.
- `bynk-model`'s crate docs and README name `WireModel` as the model's boundary half.
- Emitted output and editor behaviour are unchanged (H-1).
