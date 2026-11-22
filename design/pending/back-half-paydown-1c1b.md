---
level: patch
changelog: Internal restructuring: the duplicate-binding check, the Workers build and the events fan-out read one binding model, and the old derivations are deleted; no emitted output changes
probes: binding_kind_homes
---
refactor: the consumers switch to `WorkerBindings`, and the old derivations go (1C.1b)

Closes #4031. Part of #3676.

No output changes. The duplicate and group checks, the integration harness and the Workers build read the model `run_checks` derives once, and the events fan-out reads `plan.events`. The driver's `context_bindings`, `all_context_bindings` and their helpers, `wrangler::worker_bindings`, the `WorkersEmit` copies of the bare-resource and event-subscriber tables and the 1C.1a cross-check are deleted. The binding builders move into `bindings.rs`, so exactly one production file constructs a `BindingSource` outside the vocabulary. New gated growth probe `binding_kind_homes` (the §8 row "Places a binding kind is defined"): 3 before 1C.1, 2 after this slice, 1 after 1C.3b.
