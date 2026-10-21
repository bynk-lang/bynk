---
level: minor
probes: emit_diagnostics, diagnostic_coverage
changelog: An agent class can now be renamed with `agent New replaces Old` (its stored data and schema history move) or removed with `removed agent Old` (its stored data is deleted). Each emits a Cloudflare `exports` tombstone. `bynk deploy` plans both, and refuses a deletion until each class is confirmed by name (`--confirm-delete <context>.<Class>`); `--yes` alone never deletes. New diagnostics: `bynk.agent.replaces_unknown`, `replaces_live`, `removed_live`, `class_conflict`.
---

## ADR: agent-class-rename-and-removal
title: Agent classes are renamed with `agent New replaces Old` and removed with `removed agent Old`, emitted as `exports` tombstones and confirmed by name at deploy
summary: The declarations, the checks and lock moves, the tombstones, and the named deletion gate that let a deployed agent class change name or go away

**Context.** ADR 0443 declares every agent class in Cloudflare's `exports` map and never writes a tombstone. Its Decision 3 reserved renaming and deleting a class, both of which move or destroy stored data, for the state-migrations track (#539). Since S0 (#2205), a recorded class that disappears from source is `bynk.agent.class_removed`, and Cloudflare refuses the deploy (`orphaned_provisioned_namespace`). This increment, slice S3 (`design/tracks/state-migrations.md` §3.5, spine #2188), gives both a declared path. It comes from proposal #2248, whose decisions A–G were settled against the code and Cloudflare's class-exports reference.

**Decision.**

1. **The declarations (A).**
   - **Rename:** `agent New replaces Old`, with `replaces` contextual after the agent's name. A rename sits on the class the reader is looking at.
   - **Removal:** `removed agent Old`, with `removed` contextual before `agent` at context-item position. A removal has no class left to sit on. It is the first contextual keyword at that position: every other item starts with a reserved token, so there is no ambiguity.
   - **Where removals are kept.** They live in a side list on `Context` (`removed_agents`), not as a `CommonsItem`, because no item walk has anything to do with them.
2. **Checks (B).**
   - **Without the lock:**
     - the old name must not still be declared (`replaces_live`, `removed_live`);
     - each old name has one fate, renamed once or removed (`class_conflict`).
   - **With the lock:**
     - `replaces Old` must name a class the lock records for this context (`replaces_unknown`). An already-applied rename is the exception: an earlier build moved the entry, and the declaration stays until its tombstone is stale.
       - The entry remembers the old names it was renamed from: `renamed_from`, an additive lock field omitted when empty. So an applied rename is told apart from a mistyped `replaces` on a class with its own history, which is still `replaces_unknown` (#2253 review).
     - `New` must not have a recorded history of its own (`class_conflict`). That would merge two histories, and Cloudflare would refuse it anyway (`tombstone_renamed_to_occupied`).
   - **Settled in the build: removals of unrecorded classes stay quiet.** The settled proposal had a `removed_unrecorded` warning for a class the lock doesn't record. It is dropped. Once the build that drops an entry has run, the lock can't tell "already removed" from "never recorded", so the warning would repeat on every build until the declaration was deleted. Cloudflare documents a `deleted` tombstone for a namespace that doesn't exist as stale, with no effect.
3. **The lock (C).**
   - **A rename** moves `<context>.Old`'s entry (`version`, `floor`, `shapes`) to `<context>.New` before shapes are reconciled. So S2 steps and `__v` markers keep their version numbers, and a rename with a non-additive change still needs a step.
   - **A removal** drops the entry.
   - **No false `class_removed`.** Classes accounted for by a declaration are never reported as removed.
4. **Emission (D).**
   - **Tombstones.** Each rename emits `[exports.Old] type = "durable-object" state = "renamed" renamed_to = "New"`, and each removal emits `… state = "deleted"`. They follow the live entries, sorted by name.
   - **No old binding.** A tombstoned class gets no `[[durable_objects.bindings]]`.
   - **Renames are safe.** New's binding name changes, but nothing outside the regenerated Worker binds it, and instance names are unchanged (`idFromName` keys hold only the key value).
   - **Verified.** Wrangler 4.107.0 (`WRANGLER_MIN`) and 4.149.0 accept the emitted config in a dry run.
5. **Deploy (E).**
   - **Reading the config.** `exports` tombstones are read back as renames and deletions.
   - **Plan lines:**
     - `rename durable object Cart → Basket (moves its stored data)`;
     - `delete durable object <context>.Wishlist (all stored data; irreversible)`.

     In JSON these are `durable_objects_renamed` and `durable_objects_deleted`, omitted when empty so existing consumers see no change.
   - **A pending deletion needs its own named confirmation**, before the ordinary one:
     - typing the name at an interactive prompt, or `--confirm-delete <context>.<Class>` (repeatable);
     - `--yes` alone never deletes. That is stricter than `--prune`, because a Durable Object deletion has no Trash;
     - a confirmation naming a class the plan doesn't delete is refused, so a stale CI flag can't approve a different deletion;
     - the context name is recovered exactly from the Worker's directory name, since an identifier holds no `-`.
   - **Confirmed once (#2253 review).** A declaration stays until its tombstone is stale, so the next deploy carries the same `deleted` tombstone.
     - **The ledger records it.** `bynk.deploy.lock` records, per environment and Worker, the deletions a successful push carried, replaced on every push.
     - **Later plans** show such a deletion as `already deleted by an earlier deploy` and don't ask again.
     - **A leftover `--confirm-delete`** naming a deletion the ledger records is accepted, even after the declaration is removed, so a CI flag doesn't fail the next deploy.
     - **Why the ledger.** This records what was confirmed and pushed, not the state of a namespace, which Cloudflare owns (ADR 0194 D1). The compiler's schema lock can't serve: it changes on the first build, before any deploy.
6. **Lifetime (F).**
   - **Keep the declaration** until Cloudflare's `stale_tombstone` notice appears; Wrangler's output already reaches the user. Bynk doesn't parse it.
   - **Removing it too early is loud** (`orphaned_provisioned_namespace`).
7. **Verification and the dev-state correction (G).**
   - **Live checks** (a rename moving data, a deletion, a rename tombstone for a class the Worker never had) are recorded on #2188 before the track retires.
   - **The correction.** S0's diagnostic note and S1's guide text told authors to clear local `bynk dev` state. That was unnecessary, because `bynk dev` starts every session with empty storage, so it is removed.

**Consequences.**

- **Classes can now change.** A deployed agent class can be renamed or removed without tearing down the Worker. Only an explicit, confirmed declaration destroys data.
- **Authors must clean up.** They delete a declaration once its tombstone is stale.
- **Renaming twice takes two deploys.** A class renamed twice needs a deploy between the renames, because a tombstone's `renamed_to` must name a live class.
- **Hand-written bindings break.** A binding to an old class name in a user's own Wrangler overlay breaks on a rename; the deploy guide says so.
