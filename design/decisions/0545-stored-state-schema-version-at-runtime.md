# 0545 — Agents stamp stored state with `__v` and check it before decoding; lock-less builds carry it through; undeclared stored keys are reported once per isolate

- **Status:** Accepted (v0.375)

**Context.** The stored-state version marker ADR (S0, #2205) fixed where `__v` lives and what it means: inside the record stored under `"state"`, absent meaning version 1, and a newer marker faulting as `NewerSchema` without being written back. This increment builds it. It is slice S1 of the state-migrations track (`design/tracks/state-migrations.md` §3.2–§3.4, spine #2188), from proposal #2211, which settled the five forks below against the code.

**Decision.**

- **A. Only builds with the lock emit version logic.** Every build that touches real stored data reads `bynk.schema.lock`:
  - `bynkc compile <dir>`;
  - `bynk dev` and `bynk deploy`, through `compile_once(.., true)`;
  - `bynkc check` and `bynkc test`, read-only (ADR 0493).

  Single-file, wasm and in-memory builds have no lock and emit today's load and commit paths. They still carry a stored `__v` through unchanged, because unknown keys are spread through on both commit paths. Stamping `__v = 1` without a lock was rejected: it would fault, or downgrade-stamp, data a real build wrote.
- **B. Two numbers per class.** `reconcile_agents` returns each live class's `(version, oldest_readable)`, read from the *reconciled* entry. `oldest_readable` is the smallest `m >= floor` such that every snapshot after `m`, up to `version`, is additive (`AgentEntry::oldest_readable`). The emitter writes both as literals into the agent's `loadState`. The load rule:
  - an absent `__v` is version 1;
  - a version from `oldest_readable` to `version` loads, by zero-merge where older (ADR 0124 D4);
  - a version above `version` is `NewerSchema`;
  - anything else is `NoMigrationPath`: not a positive integer, or below `oldest_readable`.

  In S1, `NoMigrationPath` fires only on a malformed marker, because S0 refuses non-additive changes and `floor` stays at 1 until S4.
- **C. The new faults reuse `RehydrationViolation`.** Its `detail` widens to `BoundaryError | SchemaVersionMismatch { kind: "NewerSchema" | "NoMigrationPath", stored, current, oldestReadable }`. The disposition is the same: an internal fault, never a caller-facing 400. The log line adds the stored and current versions, which are not user data (ADR 0107). The check is the runtime helper `checkStoredSchemaVersion`, which constructs the fault itself. So the proposal's planned extension of the `__rehydrationViolation` import gate proved unnecessary: generated code imports only `__checkStoredSchemaVersion`.
- **D. The marker is checked first and stamped in the same write.**
  - `loadState` checks the raw stored record before the zero-merge and the `__rehydrate<Agent>State` gate, so a `NewerSchema` record is never decoded. Decoding it could pass validation by accident and then be committed back in the older shape.
  - `commitState` puts `{ ...payload, __v: <version> }` on both the encoded and plain paths, so the TypeScript state type is unchanged.
  - The test-history `__load` helper runs the same check.
  - The transition gate's raw `__prior` read runs only after a successful load, so it ignores the marker.
- **E. Undeclared keys are reported once per isolate, through `console.warn`, by name only.** `loadState` runs on every handler call, so the runtime helper `reportUnknownStoredKeys` dedupes on `(agent, key)` with a module-level set. A key counts as declared when the agent's *zero record* has it, not when it appears in a field list. So every key the toolchain itself keeps in state is declared by construction. `__v` is never reported. The key is kept and written back. A fault was rejected, because it would brick every pre-S0 deployment that holds orphaned data.

**Consequences.**

- **Rollbacks.** A rollback across an S1 build now fails loudly for each affected agent instead of silently misreading newer records. The deploy guide says to deploy forward again.
- **Existing deployments.** They gain `__v` on their next commit. That is an additive change that a pre-S1 build carries through as an unknown key.
- **Re-baselining.** Deleting a never-deployed class's lock entry (S0's way out) resets its version to 1. Local `bynk dev` state written under a higher version then faults `NewerSchema`, so S0's diagnostic note and the guide now say to clear it.
- **Later slices.** `__v` is the dispatch point for S2's `migrate from N` steps and the count S4's sweep reports.
