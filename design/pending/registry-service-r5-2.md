---
level: patch
changelog: "The first-party registry service exports itself every day (R5.2, [#2391](https://github.com/bynk-lang/bynk/issues/2391)): an hourly cron copies the bucket, each package's authoritative lines and each organisation to a backup bucket, `GET /_status/export` reports progress, and CI restores a fresh bucket from the backup alone"
---

## ADR: registry-service-r5-2
title: The first-party registry's scheduled export
summary: An hourly cron, a step a tick, copies the store bucket, each `PackageIndex`'s lines and each organisation to a `Backup` bucket; restore is a seed of the backup, drilled in CI

**Context.** Track §6 ("Durability") asks for a scheduled export of the
archives and index files, with the agents' state, so the registry can be rebuilt
after a bug or a mistake. Agents can't be listed from Bynk, and there are no
alarms. A `from cron` service, R2's paged `list`, and `traverseAll` over a page
are what exists. The accepted proposal is #2391, with organisation restore
deferred to R5a.2 on review.

**Decision.**

- **A. A `Backup` bucket** (a second `R2` capability) receives:
  - every object of the store bucket, copied when its source's etag (recorded
    in the copy's metadata) has changed;
  - `agents/packages/<name>.json`, each package's lines from a new
    `PackageIndex.snapshot()`;
  - `agents/orgs/<org>.json`, each organisation's owner, members, block and
    trusted publishers from a new `Organisation.snapshot()`, without token
    digests.
- **B. A `Directory` agent lists claimed organisations.** Every claim (and
  re-claim) adds its name, so the export can find them.
- **C. A step a tick.**
  - **The schedule.** The cron fires hourly (`17 * * * *`). A pass starts a
    day after the last one began.
  - **The steps.** Each tick copies one page of the bucket (up to 1000 objects)
    or one batch of 200 organisations.
  - **Progress.** It is kept in an `ExportRun` agent, and reported, as counts
    and times only, by `GET /_status/export`. Organisation names can't start
    with `_`, so the path can't shadow an index file.
- **D. Restore is a seed of the backup.**
  - **Packages.** Each `PackageIndex` adopts its index file when it has no
    state, so restoring the bucket restores the packages.
  - **Organisations.** Restoring them is manual until R5a.2's operator routes.
  - **Off-account copies** are the operator's job, documented in the README.
- **E. Proof.**
  - **Unit tests** (`tests/export.bynk`): a whole pass against the in-memory
    buckets, the day's wait, and copying only what changed.
  - **End to end:** `scripts/check-export.sh`, run in CI.
    1. Publish with the real client.
    2. Drive `bynk dev`'s cron until a pass finishes.
    3. Restore a fresh bucket from the backup alone.
    4. Pass the protocol's read checks against it, and resolve the package from
       a consumer.

**Deviations from the accepted proposal.**

- **A copy is matched to its source by a recorded etag, not by comparing etags
  across buckets.** An R2 etag isn't guaranteed to be a content hash (multipart
  uploads, and the test harness's buckets, differ).
- **The cron service is `nightly`, not `export`.** A service named `export`
  emits an unescaped TypeScript keyword into the generated test module. That is
  a compiler bug, reported separately.

**Consequences.**

- The registry can be rebuilt from its backup bucket with the seed tooling it
  already has.
- An export's progress is observable without access to the account.
- A large bucket takes one hour per thousand objects to pass. That's ample at
  launch, and the batch is the knob if it isn't.
