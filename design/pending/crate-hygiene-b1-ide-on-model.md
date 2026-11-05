---
level: patch
changelog: The editor's sequence diagram, wire contract and hover read handler kinds from the system model (`bynk_ide::wire_contract::BoundaryKind` is now a re-export of `bynk_ir::IrHandlerKind`, so `BoundaryKind::from_handler` is gone and an HTTP kind's `method` is an `IrHttpMethod`), and a gated `model_readers_beyond_emit` ratchet guards the model's reach (crate-hygiene B1)
---
