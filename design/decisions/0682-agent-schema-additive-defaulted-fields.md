# 0682 — The agent schema lock records each stored value's positions, and a record field added with a default is additive

- **Status:** Accepted (v0.466)

**Context.** ADR 0676 (record field defaults, #2607) made a stored record decode
a missing defaulted field as its default, but left the lock's rule unchanged (its
Decision F). The lock recorded each `store` field's type as one canonical string
(ADR 0537), so adding `size: Int = 12` to a record held in a `Cell` read as a
retyped field and failed the build with `bynk.agent.non_additive_state_change`,
although every stored instance loaded. The only way past was a `migrate` step that
copied every field by hand, while the event registry already counted the same
change as additive. Proposed and agreed in #2676.

**Decision.**

- **A. What counts as additive.** A change is additive when no `store` field is
  removed, retyped or rekinded, every recorded position still exists with the same
  shallow structure and keeps any default it had, and every new position lies under
  a new record field that declares a default (which brings what lies beneath it).
  Removing a nested field stays non-additive, because a nested record's codec drops
  unknown keys on write-back; so do a field added without a default and a new
  variant. A record field gaining a default is additive.
- **B. The lock records positions.** Each `StoreShape` gains `paths`: one entry per
  position, with `path` (`prefs.size`, `items[].note`, `byId#1.label` for a
  `store Map`'s value, `last?|Added.sku`), `type` (the node's shallow structure:
  `{}`, `|`, `variant`, `List`, `Map[String]`, a base type or `@N`), `default`, and
  `refinement` where predicates differ from the structure. The walk
  (`contract::storage_paths`) expands exactly what the canonical structure does, so
  two values have the same structure exactly when their position sets agree. A
  refinement change is read per position, because the whole-value strings differ
  whenever a field is added. The `type` and `refinement` strings are unchanged, so
  `migrate` shape matching and existing locks are unaffected.
  - **Rejected:** a marked canonical string, which would need a parser for the
    canonical form, and a whole-type tree in the lock.
- **C. Older locks.** A snapshot with no `paths` is compared by its strings as
  before. An unchanged build fills in `paths` from the declaration without bumping
  the version (the structure is identical and defaults were never recorded); a
  changed one is judged as before, with a note to build once with the recorded
  definition first.
- **D. The lock stays at version 2.** `paths` is optional. An older toolchain that
  drops it only makes the next comparison stricter, never unsafe.
- **E. A field losing its default is non-additive**, as for events: a record stored
  before the field existed would no longer decode. Changing only a default's value
  is not a shape change (ADR 0676 E).
- **F. New sum variants stay non-additive** in this increment, named as a follow-on.
- **G. Diagnostics.** `bynk.agent.non_additive_state_change` names each position
  that broke (`one (one.dark added without a default)`); the editor's contract diff
  (ADR 0640) shares the classifier and reports additive changes the same way
  (`stored state changed additively (prefs.size added with a default)`). No code is
  added or retired.

**Consequences.** Adding an optional setting to a record kept in agent state is a
one-line change with no migration. A project's lock grows a `paths` list per stored
field on its first build after upgrading, once. The runtime, emission and language
surface are unchanged: rehydration already decoded nested defaults.
