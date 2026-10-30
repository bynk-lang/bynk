---
level: minor
probes: emit_diagnostics, emit_abi_shapes, ts_writes
changelog: "`bynk.cloudflare` gains `Analytics`, a Workers Analytics Engine dataset as an instance-only derived capability with no options (`capability Signups = Analytics`), whose `write(DataPoint)` checks the platform's per-point limits before writing; `bynk deploy` derives the dataset's name in snake case (`shop_cart_signups`, `shop_cart_signups_staging`) and has nothing to create (standard-library P3b, [#2582](https://github.com/bynk-lang/bynk/issues/2582))"
---

## ADR: analytics-engine-derived-capability
title: Analytics Engine datasets as option-free derived capabilities
summary: Cloudflare's Analytics Engine binding as an instance-only `Analytics` with no options, a dataset name deploy derives in snake case because it is a SQL table name, one `write(DataPoint)` whose limits are checked before the write, and no provisioning or ledger record

**Context.** A Bynk Worker had no way to record high-cardinality events or
metrics short of an outbound `Fetch`. The standard-library track listed Analytics
Engine as a small, write-only L3 item (§6.3, §13 rule 4), split out of P3
(#2342). The binding is `env.X.writeDataPoint({ indexes, blobs, doubles })`,
synchronous and batched after the response, configured by an
`[[analytics_engine_datasets]]` stanza (`binding`, `dataset`). A dataset is
created on its first write and has no id. Per point the platform takes at most
20 blobs totalling 16 KB, 20 doubles and one index of at most 96 bytes, and a
Worker invocation at most 250 points. Track A12 gives the physical name of a
resource to `bynk deploy`, derived from the Worker and the declaration; only the
queue, whose identity is account-shared by design, is named in source. No
first-party service needs Analytics Engine today: the registry lists analytics
as a non-goal.

**Decision.**

- **A. `Analytics` is instance-only and takes no options.** An `Instantiable` row
  (prefix `ANALYTICS`, `options: &[]`, `bare: false`), as `EdgeCache` already
  has: `capability Signups = Analytics` binds as `ANALYTICS_SIGNUPS`. A bare use
  is `bynk.capability.instance_only`. Any option, including the `@dataset("…")`
  first sketched for this slice, is `bynk.capability.unknown_option`, with a note
  that deploy names the dataset.
- **B. The dataset name is derived in snake case.** `<worker>_<capability>`,
  lowercased with `-` as `_`: `shop.cart`'s `Signups` writes to
  `shop_cart_signups`. A named environment appends `_<env>`
  (`shop_cart_signups_staging`), so staging never writes into the default
  environment's dataset.
  - **Why underscores.** The dataset is the table name in the SQL API, and a
    hyphen would need quoting in every query. Cloudflare's examples use `_`.
  - **The cost.** This diverges from R2's kebab bucket names and from
    `env_qualify`'s `-<env>`, so the name has its own helpers
    (`analytics_dataset_name` in the emitter and in deploy).
  - **Not checked at plan time.** Cloudflare documents no dataset-name rule, and
    the derived name is always lowercase letters, digits and `_`.
- **C. One operation, `write(point: DataPoint)`.**
  - `DataPoint = { index: Option[String], blobs: List[String], doubles:
    List[Float] }`, a transparent record. The platform's `indexes` is an array
    of at most one, so `Option` states the rule in the type.
  - The result is `Effect[()]`: an outside-world write, though the platform call
    is synchronous.
  - **Faults, checked before writing:** more than 20 blobs or doubles, an index
    over 96 UTF-8 bytes, blobs totalling over 16 KB, or a double that is `NaN`
    or infinite. Over 250 writes per invocation faults with the platform's error,
    as `Kv.put`'s size cap does: no `AnalyticsError` yet.
  - **Rejected:** a `writeMany`. The platform has no batch call, so it would be a
    loop the caller can write. `Bytes` blobs wait for demand.
- **D. Deploy qualifies the name and does nothing else.** The `[env.<name>]`
  block qualifies each `dataset` by B's rule. There is no provisioning and no
  ledger record, so `find_orphans` and `--prune` never see a dataset. The plan
  lists each as `dataset <name> (<binding>; created on first write)`.
  - **Collisions are refused before the plan.** Flattening makes distinct
    instances meet: `shop.cart`'s `Signups` and `shop`'s `CartSignups` are both
    `shop_cart_signups`, and `shop`'s `SignupsStaging` by default is `shop`'s
    `Signups` under `--env staging`. Two instances of the build that would
    write to one dataset are refused (`analytics_dataset_problem`), in the
    environment being deployed and in every other environment the ledger has
    deployed to, as R2 refuses a shared bucket name. A collision with a
    dataset outside the build cannot be seen, since nothing records one.
- **E. Tests at every tier; experimental.**
  - **Doubles.** The `unit` double accepts every write within the limits and
    faults on one over them, as the binding does. At `integration` and `system`
    each participant's `env.ANALYTICS_<NAME>` is a fresh recorder
    (`makeIntegrationAnalytics`).
  - **Binding tests.** R-13 tests for `write`, and a `workerd` smoke that the
    binding is wired and a write doesn't fault. `wrangler dev` drops writes
    locally, so the smoke cannot show one arriving.
  - **Experimental.** `Analytics` is behind `[unstable] allow =
    ["bynk.cloudflare.Analytics"]` until a first-party service writes to it.

**Consequences.** A context records an event in one declaration and one call,
and queries it by a name it can predict from the source. A dataset with the
derived name that already exists on the account, made by hand or by another
project, is written into: data can mix, but Bynk never deletes a dataset, so none
is lost. Adopting a dataset under another name is the planned `[deploy]` table's
job (#551). Nothing stops a caller writing personal data, which the platform keeps
for three months; the reference says not to. Completion offers `Analytics` after
`capability X =`, and its hover explains how the dataset is named. Existing
goldens change only where they embed the `bynk.cloudflare` binding and adapter.
