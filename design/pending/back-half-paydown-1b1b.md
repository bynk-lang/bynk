---
level: patch
changelog: Internal restructuring of the code generator; the Bundle and Workers composition roots now read one composition plan, with no change to generated output
---
refactor(emit): units and dependencies come from `ComposePlan` (1B.1b)

Part of #3928. Part of #3676.

Byte-identical. `ComposePlan` (`bynk-emit/src/emitter/compose/plan.rs`) records what an application composes: the units, each unit's consumed contexts and surfaces, own providers and derived capabilities, the handlers' and the providers' cross-context capabilities, and which units need `__exec` or the caller. It is built once in `build_output` for both targets, in two layers: `ComposeFacts::from_tables` reads the checked tables, and `ComposePlan::decide` is a pure function over plain data, unit-tested with hand-built facts. The plan holds no spelling, no entry order and no binding type; `emit_worker_compose` still takes `&[WorkerBinding]`.

Both renderers read the plan. Each keeps its own entry order, provider spelling, `__exec` condition and cross-capability set, with a comment naming the divergence from #3928 it carries. Deleted: `worker_cross_caps`, the Bundle's `visit` closure and `context_binds_caller`, the second `{  }` site (now `compose::empty_deps`), the driver's own `ctx_uses_emit` and `ctx_uses_exec` derivations, and the two exec predicates' per-renderer walks.
