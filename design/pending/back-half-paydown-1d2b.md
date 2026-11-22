---
level: patch
changelog: Internal restructuring of `bynk deploy`; no change to plans, provisioning or `bynk.deploy.lock`
---
refactor(deploy): the apply executes the plan's typed steps (1D.2b)

Part of #4001. Part of #3676.

No output changes. `derive_plan` now decides each Worker's typed steps from its declarations and the ledger, beside the lines it prints, and holds them in the context's `WorkerSteps`, which neither plan format can see. `ResourceKind` gains `type Step`, `steps` and `apply`, and loses `provision` and `provision_all`. A step is what one ledger write covers today, so the write timing cannot move: one per KV namespace, R2 bucket, D1 database and Hyperdrive config, two per Vectorize index (its record written between them), and one per Worker for the rate limiters, the queues, each linked kind and the container images. Where a line is a view of its step (KV, R2, D1, Hyperdrive, Vectorize), it is rendered from the step, so plan and apply share one decision. The account check rewrites only lines, so it never reaches a step. The apply-time guards stay, named in each kind (`ResourceKind::GUARDS`) and pinned by a test.

`deploy_one` becomes `apply::apply_worker(run, context)`, which takes no `Resources` and walks `for_each_apply_phase`, today's apply order, pinned by a test. `run` applies each context of the plan. The plan snapshot, the ledger round-trip and the apply fixture are unchanged. This meets the track's §8 row "Deploy: apply consumes `Plan`".
