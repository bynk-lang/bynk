---
level: minor
probes: emit_diagnostics, diagnostic_coverage, ts_writes
changelog: An agent can declare `migrate from N (old: T) { … }` steps that carry a stored record from schema version N to its current shape. A step makes a non-additive `store` change buildable, runs when an older record loads (covering older versions whose path to N was additive too), and faults as `MigrationFailed` if it throws. New diagnostics: `bynk.agent.migrate_shape_mismatch`, `migrate_unknown_version`, `migrate_duplicate`, `migrate_duplicate_assignment`, `migrate_incomplete`, `migrate_impure`, and the `version_without_path` warning.
---

## ADR: agent-migrate-steps
title: Agents declare `migrate from N (old: T)` steps that carry a stored record from version N to the current shape, checked against the lock and run at load
summary: The migrate surface (front-loaded by the state-migrations track): a named old record type, a pure `:=` body with record-literal semantics, coverage rules, additive-predecessor ranges, and the load-time dispatch

**Context.** S0 (#2205) made a non-additive change to an agent's stored state a build error, and S1 (#2213) stamped every stored record with its schema version (`__v`). This increment, slice S2 of the state-migrations track (`design/tracks/state-migrations.md` §3.3–§3.4, as re-settled by #2227), gives such a change a way through. It comes from proposal #2224, whose decisions A–F the maintainer settled.

**Decision.**

1. **Surface (A, B).** An agent member `migrate from N [(old: T)] { item* }`, declared between the `store` fields and the handlers. `migrate` and `drop` are contextual keywords.
   - **`T` is an ordinary named record type** naming the version-`N` fields the step reads. `TypeRef` has no anonymous record form, and record literals always name their type.
   - **An item** is `f := e`, `let x = e` or `drop f`.
   - **Record-literal semantics for `:=`.** Each current field may be assigned at most once, in any order. A bare field name reads its carried value, never an earlier assignment.
   - **Assignable kinds.** `:=` accepts `Cell` and `Map` fields. `Set`, `Log` and `Cache` fields have no value type, so they can be neither read nor assigned.
2. **Checking against the lock (A, C)**, in `reconcile_agents`.
   - **Version.** `N` must be recorded, at or above the floor and below the current version.
   - **Shape.** Each field of `T` must match version `N`'s field of that name on structure: `T` matches a `Cell` of `T`, and `Map[K, V]` matches a `Map` of `K, V`. A difference only in refinements warns with `refinement_changed`.
   - **Drops.** Each `drop f` must name a field of version `N` or of the current shape.
   - **Coverage, against the current shape:**
     - each removed field must appear as `old.f` in the body, or be dropped;
     - each field whose structure or storage kind changed must be assigned;
     - a `Set`, `Log` or `Cache` field (on either side) may be dropped instead, which starts it at its initialiser or zero.
   - **Accepting a change.** A non-additive change into `K` is accepted, recorded with `additive = false`, exactly when a step from `K-1` passes.
   - **Every step, every build.** Coverage is checked for every step on every build, not only at its transition. A later change that a step no longer accounts for is therefore an error in that step, rather than a silent zero.
3. **Additive predecessors (C).**
   - **The range.** A step from `N` covers every version `M`, at or above the floor, whose path `M` → `N` is entirely additive (`AgentEntry::oldest_readable_to`).
   - **Missing old fields.** A field of `T` missing from such a record takes `T`'s field default, else its type's zero. To allow this, a record type named by a step's `old` is exempt from `bynk.event.default_outside_event`, and its defaults are validated as event defaults are.
   - **`version_without_path`.** It warns for a version from the floor to current that no step's range covers and that has no all-additive path to current.
4. **Purity (E).** A step is checked like an invariant: no effects, `?`, `expect` or capability calls, and no reference to the agent's key (`migrate_impure`). A sweep wakes an instance by object ID, which does not recover the key.
5. **At load (D).** `loadState` runs these in order:
   1. S1's `__v` check, which now also accepts versions inside a step's range.
   2. A dispatcher that runs the **nearest** step whose range covers the stored version: the one with the smallest `N` at or above it.
   3. Steps take precedence over the zero-merge. So a step written for a version reachable additively, such as a default-on-read step for a narrowed refinement (which the lock records as additive), still runs.
   4. The step decodes `T` from the raw record through `T`'s own codec, and rebuilds the carried state: fields whose structure is unchanged, decoded as today, with every other field at its zero.
   5. It evaluates the assignments.
   6. It encodes the result, and keeps any key version `N` did not record.
   7. The result is then zero-merged and passes the ADR 0124 gate like any record.
   8. A throw inside the assignments is `RehydrationViolation` with detail `MigrationFailed { from }`. The thrown value is never logged, and nothing is written.
6. **Without a lock (F).** Steps are type-checked but emit nothing, because no versions exist to dispatch on.

**Consequences.**

- **Breaking changes can ship.** A non-additive change to a deployed agent's state is now possible without hand-staging through handlers. The `owner → ownerName` reproduction from #539 builds and migrates. A workerd smoke covers state written by one build and read through a step by the next.
- **Old steps stay honest.** Every step is re-checked against each later change, so an old step can't silently zero a field a later change retyped. The cost is that old steps need editing when later changes touch the fields they cover, until S4's sweep raises the floor and retires them.
- **Map conversion.** A `store Map` persists as a `Record<string, V>`, while a `Map` value is a `ReadonlyMap`. The step converts at its edges: the stored record becomes entries for `T`'s decoder, and an assigned map becomes a record.
- **S0's rename diagnostic** now names a concrete step, `type CartV1 = { owner: String }` with `migrate from 1 (old: CartV1) { ownerName := old.owner }`, when the stored type is a plain name.
