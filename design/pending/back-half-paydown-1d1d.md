---
level: patch
changelog: Internal restructuring of `bynk deploy`; no change to plans or `bynk.deploy.lock`
probes: deploy_kind_accessors
---
refactor(deploy): rate limiters and queues as `ResourceKind`s (1D.1d)

Part of #3929. Part of #3676.

No output changes. Rate limiters (`deploy/kind/ratelimit.rs`) and queues (`deploy/kind/queue.rs`) implement `ResourceKind`. A rate limiter's id is allocated by the ledger, recorded for all of a Worker's limiters in one write before the push, and a limiter the source dropped is forgotten once the push succeeds, through the new `ResourceKind::after_push`; it reports no orphans (`NoOrphan`). A queue's records are environment-wide; its `provision_all` makes every queue the run has not yet attempted before recording any, as before, and a queue nothing names is an orphan `--prune` deletes unless it was only reused. `find_orphans` is now the kind list and the Worker block, which sits in the list as a `Slot` so the orphan report keeps its order. `allocate_ratelimit`, `forget_undeclared_ratelimits`, `recorded_ratelimit`, `first_unrecorded_ratelimit`, `has_queue`, `record_queue` and `record_reused_queue` are deleted. The byte-identity fixtures are unchanged. `deploy_kind_accessors` falls from 25 to 11, `run`'s ceiling from 720 to 716, and `deploy_one`, now under 300 lines, leaves the ceiling list.
