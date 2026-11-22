---
level: patch
changelog: Internal restructuring: one typed binding model, `WorkerBindings`, is derived once per build from the composition plan, beside the old derivation it is checked against; no emitted output changes
---
refactor: a typed `WorkerBindings` model, derived once per build (1C.1a)

Part of #4031. Part of #3676.

No output changes. `bynk-emit` gains `WorkerBindings` (`emitter/bindings.rs`): each context's bindings in the order its `wrangler.toml` lists them, and each Worker's merged list. The order is one exhaustive `match`, so a new `BindingSource` fails to compile until it is placed. `ComposePlan` records the facts that imply a binding (`called`, `bare_kv`, `bare_singletons`, `idempotency`), and the provider walk behind them moves from `bynk-driver` into `bynk-emit`. The plan and the model are built once, in `run_checks`, and carried to `build_output`, which builds no second plan. No consumer is switched yet: a `debug_assert_eq!` compares the model with the old derivation on every fixture, until 1C.1b switches the consumers and deletes it.
