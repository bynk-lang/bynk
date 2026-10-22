# 0566 — A sweep raises a floor only after the swept Workers' last code deployments have settled for 30 minutes, and pushed renames are recorded as applied

- **Status:** Accepted (v0.383.2)

**Context.**
- **The listing lags.** The live checks for the state-migrations track (#2188) found that Cloudflare's Durable Object objects listing shows a newly stored instance 5 to 11 minutes after its first write.
- **That breaks the floor rule in one case.** The floor rule from the agent migration sweep ADR (#2259, Decision F) assumes the listing is complete. An instance the previous build first stored just before a deploy can be missing from a sweep run straight afterwards. That sweep would raise the floor past the instance, which would fault with `NoMigrationPath` once a later build deletes the step.
- **An applied rename read as pending.** The same run showed that a rename a deploy had already applied still read as pending in later plans, unlike a deletion.
- **Source.** This comes from proposal #2276, whose decisions A–E were settled against the code and a live probe of the deployments API.

**Decision.**

1. **The deploy time (A).**
   - **Which deployment counts.** For each swept Worker, the sweep reads Cloudflare's deployments list before setting the run's key. It takes the newest deployment whose `workers/triggered_by` isn't `secret`. Any other or absent value counts as code, including `upload`, `deployment` and `rollback`.
   - **Paging.** The list is paged (10 a page by default), and each sweep adds two `secret` deployments, so the sweep pages back, 50 at a time and up to 1,000, until a code deployment is in view.
   - **When none is in view.** If every deployment it read is a secret change, the oldest read stands in, because the code is at least that old.
   - **The clock.** `created_on` is parsed as RFC 3339 and compared with the local clock. A time ahead of it counts as just deployed.
2. **The margin (B).** A fixed 30 minutes, about three times the longest lag observed, with no flag to shorten it.
3. **Inside the margin (C).**
   - **The run still migrates.** It keeps every class's floor, and says which Worker was deployed how long ago and how many minutes remain.
   - **A failed read holds the floor.** A deployments call that fails holds the floor the same way.
   - **The age is the listing's.** It is measured from when the listing is taken.
4. **Applied renames (D).**
   - **The record.** `bynk.deploy.lock` records, per environment and Worker, the `old → new` renames a successful push carried, replaced on every push, as it already does for deletions.
   - **The plan.** A recorded rename reads `already applied by an earlier deploy`, and its JSON entry carries `already_applied`.
5. **Level (E).** Patch: no language surface changes.

**Consequences.**
- **A floor can't rise early because of the listing's lag.** A sweep run soon after a deploy migrates but has to be repeated for the floor to rise.
- **The rule depends on two Cloudflare behaviours.** It relies on the `workers/triggered_by` annotation, and on the lag staying well inside 30 minutes. A change to either holds the floor rather than raising it early, except a lag past the margin, which would mean revising the constant.
