---
level: patch
changelog: Internal restructuring: the deploy `plan.rs` splits into its run, plan model, keys, contract-skew policy and messages; no output changes
---
refactor: split `bynk/src/deploy/plan.rs` (2.8)

Closes #4109. Part of #3676.

No output changes. `plan/run.rs` takes the run (`DeployOptions`, `run`, `deploy` and the helpers that select and record), `plan/model.rs` the plan (its types, `derive_plan`, `plan_report` and the confirmations), `plan/keys.rs` the managed and edge keys, `plan/skew.rs` the contract-skew policy and its refusals, and `plan/messages.rs` the messages a run prints. The tests move whole to `plan/tests.rs`, under the same `plan::tests` path. `plan.rs` becomes a 51-line facade that re-exports what `deploy` names, and 2.0's ceiling entry for it is deleted. No gated probe moves.
