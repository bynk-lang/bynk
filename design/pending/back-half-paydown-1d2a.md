---
level: patch
changelog: Internal restructuring of `bynk deploy`; no change to plans, provisioning or `bynk.deploy.lock`
probes: deploy_ledger_writes
---
refactor(deploy): one ledger write site, `LedgerWriter` (1D.2a)

Part of #4001. Part of #3676.

No output changes. A kind no longer writes `bynk.deploy.lock`: its provisioning and pruning read the ledger through a read-only `StepCx` and return what they would record or forget as `Commit`s of typed changes (`TableChange`, and `HyperdriveChange`, `VectorizeChange` and `QueueChange` for the kinds with their own forget or "only when it changed" rule). `deploy/apply.rs`'s `LedgerWriter` makes each commit's changes and writes the ledger, before the kind's next account call, so ADR 0180's "recorded before the push" is held in one place. What one write covers is unchanged; Vectorize makes an index's metadata indexes only once its record is written, as before. The post-push record (`record_pushed`) and `--prune` go through the same writer.

A new fixture pins what the apply does, added before any code moved: the every-kind project applied in both environments against the every-kind ledger, with its account calls, each ledger write and the call it followed, and the run again with each account-changing call and each write failing. It, and the plan and ledger snapshots, are unchanged. `deploy_ledger_writes` is a new gated growth probe; it read 25 before this slice and reads 1 after it. `plan.rs::run` falls from 716 to 709 lines.
