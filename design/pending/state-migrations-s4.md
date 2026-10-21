---
level: minor
probes: emit_diagnostics, ts_writes, diagnostic_coverage
changelog: "`bynk migrate status` shows each agent class's version, floor, migrate steps and the versions they cover, and where it is deployed. `bynk migrate sweep` wakes every stored agent instance in the deployed Workers so each migrates, then raises a class's floor when one run covered every environment it is deployed to; a step below the floor warns `bynk.agent.migrate_below_floor` and can be deleted."
---

## ADR: agent-migration-sweep-and-floor
title: `bynk migrate sweep` wakes every stored agent instance through a per-run signed route, and raises a class's floor only after one complete run over every deployed environment
summary: How the toolchain finds, wakes and migrates stored agent instances, and when that evidence lets the lock's floor rise so old migrate steps retire

**Context.**
- **Lazy migration leaves steps behind.** Since S2 (#2224), a stored agent record migrates when it is next loaded. That is enough for correctness, but an author can delete a `migrate from N` step only once no stored instance can still hold version `N`.
- **The floor exists but can't move.** The lock's `floor` (S0) exists for this, and track §3.3 left raising it to S4, slice S4 of the state-migrations track (`design/tracks/state-migrations.md` §3.6, spine #2188).
- **Two things were left to S4.** Track §3.6 settled that the sweep route must be authenticated by a toolchain-controlled credential, and left the mechanism open. It didn't say how one floor shared by every environment relates to data stored per environment.
- **Source.** This comes from proposal #2259, whose decisions A–H were settled against the code and Cloudflare's API reference and limits.

**Decision.**

1. **The command (A, H).**
   - **`bynk migrate status`** is offline. It checks the source against the lock read-only and reports each class's version, floor, valid steps and the versions they cover, versions with no path, and the environments `bynk.deploy.lock` records its Worker in.
   - **`bynk migrate sweep`** wakes deployed instances. `bynk dev` is out of scope, because its storage is empty on every start.
2. **Cloudflare access (B, C).**
   - **The REST API**, through `ureq` over rustls (ring) with the operating system's certificate roots. The CDLA-licensed root bundles stay out of the tree.
   - **Credentials** come from `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID`. The token needs "Workers Scripts: Edit", because `wrangler` uses the same token for the run's secret.
   - **Namespaces** are found by listing them and matching `script` and `class`. Nothing about them is recorded.
   - **Objects** are listed with paging, and only those with `hasStoredData` are woken.
3. **The wake (D).**
   - **The Worker route.** A Worker hosting a versioned agent answers `POST /_bynk/migrate/<Class>` with `{ ids }`. With the secret `BYNK_MIGRATE_KEY` unset, it answers `404`. It admits the caller `bynk.migrate` only, so a context's edge key can't use it. It then verifies the published call signature under that secret.
   - **The Durable Object branch.** The Worker wakes each id with `idFromString` through a reserved `/_bynk/migrate` branch of the Durable Object's `fetch`, ahead of the method dispatch.
     - The branch carries no key. It runs `loadState`, which applies any step.
     - It calls `commitState`, with its invariant gate, only when the stored `__v` was older.
     - It reports `{ before, after }`, or a fault by kind alone.
   - **The answer** also states `current`, the version the deployed Worker writes.
   - **The key.** Each run mints a key per Worker and sets it with `wrangler secret put --name <worker>` on stdin. It deletes the key afterwards, including on failure and Ctrl-C. A failed delete prints the command to finish it.
4. **Batching (E).**
   - **Batch size.** 40 ids per request by default (`--batch`), within the Free plan's 50 subrequests.
   - **Readiness.** An empty first request waits for the new secret to reach the Worker.
   - **Retries.** A batch whose request fails is resent once. Afterwards, each id that came back with a transport fault (`Unreachable`, or a status) is woken once more, which is safe because a wake is idempotent. A load fault is reported and never retried. A fault left after this leaves the run incomplete.
5. **The floor (F).**
   - **Nothing is recorded.** How old stored instances are is Cloudflare's state, so it stays out of the ledger (ADR 0194 D1).
   - **When it rises.** One run raises a class's floor only when it completed every environment the ledger records the class's Worker in.
   - **How far.** It rises to the minimum of every woken instance's version and every environment's `current`, never past the lock's own version.
   - **The second bound matters.** Without it, an environment with no stored instances running older code could create instances below the new floor.
   - **What a rise does to the lock.** It drops the lock's snapshots below the floor.
6. **Steps below the floor (G).**
   - **The test.** `migrate from N` with `N` below the floor warns `bynk.agent.migrate_below_floor` and emits nothing.
   - **Why `N < floor` is enough.** Versions are contiguous from 1, so every such `N` was recorded, even after its snapshot is gone.
   - **What doesn't warn.** `version_without_path` doesn't warn below the floor.

**Consequences.**
- **Steps can retire.** Authors can delete `migrate` steps on evidence rather than guesswork.
- **No standing capability.** The sweep leaves nothing standing: no key on disk or in the ledger, and no route answering between runs.
- **What it costs.** A sweep costs a request per stored instance and two Worker versions per Worker.
- **Rolling back.** After a floor rise, a rollback below the floor would store new instances at a version no step covers. The guides say to roll back no further than the floor.
- **Invisible environments.** An environment deployed outside the committed ledger is invisible to the floor rule.
- **`sweep` is gated.** It relies on Cloudflare's objects listing covering SQLite-backed namespaces, the live check recorded on #2188. If that check fails, `sweep` is withdrawn and `status` stays.
