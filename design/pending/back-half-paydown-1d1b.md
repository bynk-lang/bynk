---
level: patch
changelog: Internal restructuring of `bynk deploy`; no change to plans or `bynk.deploy.lock`
probes: deploy_kind_accessors
---
refactor(deploy): R2 and D1 as `ResourceKind`s (1D.1b)

Part of #3929. Part of #3676.

No output changes. R2 (`deploy/kind/r2.rs`) and D1 (`deploy/kind/d1.rs`) implement `ResourceKind`: their plan and short lines (D1's migrate line with it), the account listing and check, the name problems, their orphans (superseded records included), provisioning, D1's materialisation and the D1 id the deploy facts and `bynk revert` read all come from the trait. A record that replaces another keeps the old one as superseded through the kind's `LedgerTable`, as `record_r2` and `record_d1` did. `recorded_r2`, `recorded_d1`, `record_r2`, `record_d1`, `provision_r2_buckets` and `provision_d1_databases` are deleted, and the helpers move into the kinds with their tests. Vectorize, not yet converted, keeps its place in the plan between them through a `Legacy` slot in the fixed kind list. The byte-identity fixtures are unchanged. `deploy_kind_accessors` falls from 54 to 43.
