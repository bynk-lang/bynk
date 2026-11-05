---
level: patch
changelog: The system model's scope and placement are recorded (crate-hygiene B1), and every handler kind's and service protocol's editor rendering is pinned by a snapshot
---

## ADR: system-model-scope-and-placement
title: The declaration-level IR becomes the system model, above the checker, with `bynk-lower` merged in as `bynk-model`
summary: The system model's scope, placement and name, its M-1 to M-4 requirements, and the M-1 domain clause

**Context.** `bynk-ir` holds the declaration-level IR that Part 15.1 of
`design/bynk-greenfield-compiler.md` adopted: protocols, handler kinds and routes,
type shapes, store fields, capability and attached-fn signatures, `given`
references, actor seams and event subscriptions. `bynk-lower` builds it.

Only `bynk-emit` reads either crate. Every other tool that needs the same facts
re-derives them from the AST. That includes the editor's sequence diagram, wire
contract and hover; `bynk openapi`; and the smoke runner.

The crate-hygiene track (`design/tracks/crate-hygiene.md` §6, spine #2775) settled
where the model sits (Q1), whether `bynk-lower` stays separate (Q2), its name (Q3)
and whether it is a public interface (Q4). Slice B1 (#2939) tests the result: it
moves `bynk-ide` onto the model's handler kinds. This record carries those answers
and the requirements every later slice is checked against.

**Decision.**

- **A. Scope.** The crate is the **system model**: the resolved description of
  what a Bynk program *is* at the level of declarations. Every tool that needs
  those facts reads them from the model, not from the AST. `bynk-lower` stays the
  builder from a checked program to the model.
- **B. What the model is not.**
  - **Not an expression IR.** Bodies stay where Part 15.1 put them, and that
    entry's trigger is unchanged. A model item may *summarise* a body
    (`block_uses_emit`, `body_writes_state`), but never represents one.
  - **Not the emit ABI.** ADR 0310's four items describe the emitted TypeScript.
    The model is a Rust-side description of the source program.
  - **Not a second checker.** The model is built after `certify` and carries no
    diagnostics of its own.
- **C. Placement (Q1).** Above `bynk-check` for slices B1–B3. Every consumer
  already depends on the checker, so no serial build step is added. B4 decides
  whether to move it below, on what B1–B3 show. If it moves below, the first step
  is taking `block_uses_emit` off `bynk-check`.
- **D. `bynk-lower` merges in (Q2).** It is the model's only constructor. Merging
  it removes one step from the build chain.
  - The `unconsumed_ir_items` probe's two-owner rule becomes a one-crate rule:
    every public item of `bynk-model` needs a production reader outside the crate.
  - If B4 moves the model below the checker, the builder splits back out.
- **E. Name (Q3).** `bynk-model`, applied in its own behaviour-neutral PR once B1
  passes. The crate is named for what it produces (R10.1). Its docs say in one
  line that it is distinct from `bynk_check::project_model`, which models units
  and manifests, not declarations. `bynk-ir` and `bynk-lower` each get a final
  release that names `bynk-model`.
- **F. Requirements.**
  - **M-1 One source per fact.** A consumer that matches the AST for a fact the
    model carries is a defect.
    - **Domain clause** (B1, #2939): M-1 applies where the consumer holds the
      model's input. The model is total over certified programs (M-2). A consumer
      answering over an uncertified, recovering parse may read the AST for a fact
      whose lowering needs a checked program.
    - A source-spelling renderer is a printer, as `bynk-fmt` is, not a model
      reader.
    - Each such site cites this clause in a comment.
  - **M-2 Total over certified programs.** A gap is a missing variant, not a
    fallback. The existing `Ty::Unit` fallbacks in `bynk-lower` are documented
    exceptions, to keep or retire, not a precedent.
  - **M-3 Every item has a reader outside the model.** `unconsumed_ir_items` gates
    this. A second gated probe, `model_readers_beyond_emit`, counts the model items
    with a production reader outside `bynk-emit`. A floor test makes that count
    able only to rise, whatever a pending file's `probes:` acknowledges.
  - **M-4 No new AST embedding.** The two documented embeddings stay:
    `TypeShape::Refined` carrying `BaseType` and `Refinement` (ADR 0366). A new
    embedding needs the same written justification.
- **G. B1's exit criterion, judged mechanically.** It is measured over non-comment
  production lines of `bynk-ide/src` (`sequence.rs`, `wire_contract.rs`,
  `symbols.rs`, `hover.rs`).
  - **C1a.** AST `HandlerKind::`/`HttpMethod::` lines go to 0. `ServiceProtocol::`
    lines fall under the domain clause.
  - **C1b.** No equivalent IR-variant matches appear elsewhere:
    - every added `IrHandlerKind::`/`IrHttpMethod::` line sits in the function
      that held the AST match it replaces
    - no `bynk-ide` type mirrors a model enum variant for variant
    - no `bynk-ide` function lowers an AST variant to a model value
  - **C2.** No model item is read only by the IDE.
- **H. `BoundaryKind` goes.** `bynk_ide::wire_contract::BoundaryKind` mirrored
  `HandlerKind` so that "a future field this peek needs is a local addition". It is
  replaced by a re-export of the model's `IrHandlerKind`. Under M-1, a field the
  peek needs becomes a field of the model, where it gets a second reader for free.
- **I. Public interface (Q4).** Deferred until B3. So that no slice closes the
  option, model items stay plain data, with no lifetimes and no references into
  the AST.

**Rejected.**
- **Folding `bynk-ir` and `bynk-lower` into `bynk-emit` now.** This is B1's
  failure path. It would raise the gated `ast_importers` count and cut the IDE off
  the model, because `ide_emit_edge` must stay absent.
- **Applying M-1 to every IDE site without the domain clause.** Hover, the
  documentation page and the system-job hover answer over a parse that need not
  certify. `ProtocolIr` carries resolved `TyId`s and can't be built there. It also
  doesn't carry the source spelling that hover prints, and M-4 forbids embedding
  it.
- **Keeping `BoundaryKind` and building it from `IrHandlerKind`.** That is an
  equivalent eight-arm match left in the IDE, which C1b fails.

**Consequences.**
- `bynk-ide` gains a `bynk-lower` edge (allowed by the track's §3; `ide_emit_edge`
  stays absent). The longest serial chain is unchanged, and the Q2 merge later
  shortens it by one step.
- Under the domain clause, three sites in `symbols.rs` stay on the AST, each
  citing the clause:
  - `service_protocol_suffix` (6 `ServiceProtocol` arms)
  - the `is_job_service` probe (2 `ServiceProtocol` tag tests)
  - `cache_control_of`, the per-annotation `@cache` reading
- B2 (`bynk openapi` and the smoke runner) and B3 (entry signatures) hold certified
  programs, and are where the *resolved* half of the model is tested.
- A B1 pass shows that the model can be the IDE's single source for facts that
  exist at parse level. `IrHandlerKind` mirrors `HandlerKind` field for field, so a
  pass doesn't show that the resolved items serve the IDE. That question is B4's.
- Part 15.1's IR entry now names the crate-hygiene track as where the adopted half
  grows.
