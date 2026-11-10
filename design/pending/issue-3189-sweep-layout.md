---
level: minor
changelog: `bynk migrate sweep` reports storage-layout progress once a deployed Worker writes a layout beyond the original one (per-entry storage, slice 5a)
---

## ADR: migrate-sweep-reports-layout
title: The sweep wake reports each instance's storage layout, and the sweep says how far the layout upgrade has got
summary: The wake answer carries layoutBefore and layoutAfter, the sweep answer carries the layout the Worker writes, and the sweep line reports instances moved up; none of it affects the floor

**Context.** The per-entry storage track (`design/tracks/per-entry-storage.md`, #3195, proposal #3189) adds a stored-layout marker, `__layout`, with a lazy upgrade in `loadState`. `bynk migrate sweep` already wakes every stored instance through `/_bynk/migrate`, which calls `loadState` and `commitState`, so it is the eager path for the upgrade and needs no new mechanism. What it lacks is a way to say how far the upgrade has got, and an operator wants that before relying on the new layout.

**Decision.** The Worker's wake answer for one instance gains `layoutBefore` (the layout its record was in: 1 when `__layout` is absent, the number when it is a positive integer, null otherwise) and, when the wake succeeds, `layoutAfter` (the layout this build writes). The sweep's answer gains `layout`, the layout this build writes. `wakeForMigration` and `sweepAgentInstances` take it as an optional last argument that defaults to 1, so a build whose emitted code does not pass one reports layout 1 and the sweep line is unchanged. The driver reads all three fields as optional, so a Worker built before this increment still sweeps. Once a Worker reports a layout above 1, the sweep line gains `layout N: X moved up, Y already there`, counting woken instances whose record was in an older layout. A record in a layout the build does not know faults `NewerLayout`, which the existing per-instance fault report already names. **The layout is informational only:** it does not enter the rule for when a class's floor may rise, which stays a question about schema versions. `bynk migrate status` is not changed here: it reads the lock, not stored records, so it has no layout to show until a build records one.

**Consequences.** No behaviour changes today: no build writes a layout above 1, so no sweep line changes and nothing else reads the new fields. The emitter starts passing the layout it writes when the layout change itself lands. Hover, completion, semantic tokens and signature help are unchanged, because no syntax, name or signature changes.
