---
level: patch
changelog: Internal restructuring of the code generator; the Bundle and Workers composition roots now live together in bynk-emit, with no change to generated output
---
refactor(emit): the compose functions move into `bynk-emit::emitter::compose` (1B.1a)

Part of #3928. Part of #3676.

A mechanical move, byte-identical. `emit_worker_compose`, `fanout_events` and `worker_cross_caps` leave `workers.rs`; `emit_composition_root` and its helpers leave the driver. Function bodies are unchanged; no `ComposePlan` yet (1B.1b and 1B.1c).
