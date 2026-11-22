---
level: patch
changelog: Internal restructuring of `bynk deploy`; `bynk deploy --prune` now lists and deletes orphans in the plan's kind order (KV, Vectorize, Hyperdrive, then queues); plans and `bynk.deploy.lock` are unchanged
probes: deploy_kind_accessors
---
refactor(deploy): Vectorize and Hyperdrive as `ResourceKind`s (1D.1c)

Part of #3929. Part of #3676.

Vectorize (`deploy/kind/vectorize.rs`) and Hyperdrive (`deploy/kind/hyperdrive.rs`) implement `ResourceKind`: plan and short lines, the account listing and check, the index-name problem, orphans with superseded and adopted records, prune notices and pruning, provisioning, and Hyperdrive's materialisation. `ResourceKind::Live` is now the run's account state as a kind sees it, taken from one `Lives` bundle: KV's live ids, and Hyperdrive's wants and API (its `HyperdriveRun`), which the plan line, provisioning and `--prune` read. A kind records through `ApplyCx::record` and `ApplyCx::change`, which write the ledger at once, so Vectorize's record still lands before its metadata indexes are made (ADR 0180). `recorded_vectorize`, `recorded_hyperdrive`, `record_vectorize`, `record_hyperdrive`, `forget_vectorize`, `forget_hyperdrive`, `hyperdrive_plan`, `Plan::with_hyperdrive` (now `Plan::with_lives`), `hyperdrive::prune_run` and the free provisioning functions are deleted. The `Legacy::Vectorize` slot from 1D.1b goes.

One ruled change (the maintainer, 22 November 2026): `--prune`'s "will delete" notices and its deletes follow the one kind list's order (KV, Vectorize, Hyperdrive, then queues), where they had orders of their own (notices: KV, queues, Hyperdrive, Vectorize; deletes: KV, Vectorize, queues, Hyperdrive). Plan output in both formats, `bynk.deploy.lock` and the byte-identity fixtures are unchanged. Hyperdrive's CI refusal keeps its place after the rate limiter's, outside the list, until 1D.1d. `bynk revert` still does not check a Hyperdrive id (#3970). `deploy_kind_accessors` falls from 43 to 25.
