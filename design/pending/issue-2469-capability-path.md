---
level: minor
changelog: Hovering a `given` entry shows why the declaration needs the capability, the shortest chain of agent-handler calls to its use, and a **Show capability path** action lists every chain (#2469)
---

## ADR: capability-path-witness
title: The checker records one witness path per `given` entry; the editor never re-derives it
summary: Shortest agent-call chain per (given entry, capability), full set computed on demand

**Context.** A `given` clause is an honest summary of what a declaration
touches, but when a capability is reached through other bodies the editor could
say *what* was missing (the ghost `given` hint) and *what* was unused
(`bynk.given.unused_capability`) and not *why* a capability is required. The
only edges that carry a capability between bodies are agent-handler calls
(`Agent(key).m()`, whose deps are widened, #527, and which a provider must list
in its `given`); plain functions cannot declare `given`. Re-deriving that walk
in the language server would duplicate the checker's call classification and
drift from it.

**Decision.**

- The checker owns the answer. After a unit's bodies are checked, the same pass
  that validates provider agent calls builds a per-file `CapabilityGraph` from
  the requirement ledger's covered uses and the `Callee::Agent` classification.
- It records **one shortest witness** per `(given entry, capability)` and keeps
  only the compact call/use facts behind it. The full set of chains is computed
  **on demand** (`all_paths`, shortest first, capped at 64), never stored per
  entry, so memory is bounded by the number of call and use sites.
- The editor reads it: hover on a `given` entry appends the witness under the
  capability's description, each step a link; the **Show capability path** quick
  fix carries a client command that opens the references peek over every step.
  The action is offered only to a client advertising
  `experimental.capabilityPathCommand`, as the Explain action is.
- The issue proposed opening the call hierarchy pre-filtered. The LSP call
  hierarchy has no filter hook, and its graph is the symbol call graph rather
  than the capability one, so the references peek is used instead; the steps are
  the same sites.

**Consequences.** No language, grammar, emitter or runtime change. A witness
exists only for an entry the body reaches; an unused entry has none, which
`bynk.given.unused_capability` already reports. The graph is built from the
unit's table, so a path crosses files within a context but not units. Tooling
delta: hover changed (`given` entries show the path); a code action added;
completion, semantic tokens and signature help unchanged.
