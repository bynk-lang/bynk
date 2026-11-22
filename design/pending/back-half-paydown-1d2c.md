---
level: patch
changelog: Internal restructuring of `bynk deploy`; no change to plans, provisioning or `bynk.deploy.lock`
---
refactor(deploy): `run` as orchestration (1D.2c)

Part of #3676. Closes #4001.

No output changes. `plan::run` was 709 lines, the longest function on the deploy path. Its body is now `deploy`, 233 lines of gates, the plan, the confirmation, the apply and `--prune`, each step returning the code the run exits with, chained with `?`; `run` is the wrapper that turns that into an `ExitCode`, and its ceiling entry is removed. The blocks moved out unchanged (every message, and the order it is printed in, as before): the `--context` contract-skew gate, the upload order and its cycle report, the foreign callers' pins, the edge and managed keys, the opening preflight and the account checks before the confirmation into `deploy/gates.rs`; the apply loop and `--prune` into `deploy/apply.rs` (`apply_plan`, `prune_plan`). The plan snapshot, the ledger round-trip and the apply fixture are unchanged.
