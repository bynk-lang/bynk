---
level: patch
changelog: Internal restructuring of `bynk deploy`; no change to plans or `bynk.deploy.lock`
probes: deploy_kind_accessors
---
refactor(deploy): the `ResourceKind` trait, the fixed kind list, and KV (1D.1a)

Part of #3929. Part of #3676.

No output changes. `deploy/kind.rs` holds the typed `ResourceKind` trait (associated types for a kind's declaration, ledger record, ledger table, plan line, orphan and run-wide account state), `KindId`, `LedgerTable`, the contexts a kind works through (`LedgerView`, `PlanCx`, `ProjectCx`, `OrphanCx`, `ApplyCx`, `Tools`), and `for_each_kind`, the one fixed list. KV is the first kind (`deploy/kind/kv.rs`): its plan line and short line, the account check and its note, the title-collision and CI checks, its orphans, prune notice and prune, the live-id read, provisioning and materialisation, and the id its deploy facts and `bynk revert` read all come from the trait, and `recorded_kv`, `recorded_kv_binding`, `record_kv`, `forget_kv` and `first_unrecorded_kv` are deleted. `on_account::Kind` folds into `KindId`. The ledger keeps its two KV maps, so it is written byte for byte as before.

Two fixtures pin this: a ledger recording every kind in two environments round-trips byte for byte, and the short and JSON plans of a project declaring every kind, against that ledger, are unchanged. `deploy_kind_accessors` is a new gated growth probe; it read 68 before this slice and reads 54 after it.
