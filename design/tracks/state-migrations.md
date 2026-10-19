# State migrations: evolving a deployed agent's persisted state

- **Status:** Settling. Spine issue [#2188](https://github.com/bynk-lang/bynk/issues/2188)
  is open. This doc arrives in a draft settling PR, and nothing is built yet. Scopes
  design-review issue [#539](https://github.com/bynk-lang/bynk/issues/539) (schema
  fingerprint, `migrate` transform, rename detection, agent enumeration).
- **Realises:** Gate 3 of [`../bynk-1.0-definition.md`](../bynk-1.0-definition.md)
  ("state migrations"), the one unmet 1.0 gate, and the second adoption blocker in
  [`../bynk-adoption-sequencing.md`](../bynk-adoption-sequencing.md). It builds what
  [ADR 0124](../decisions/0124-rehydration-validation-and-migration.md) D5 deferred: a
  versioned-schema migration capability, per-field default-on-read, and a soft
  rehydration handler. It also takes the class rename and deletion work that
  [ADR 0194](../decisions/0194-deploy-queues-and-delegated-do-migrations.md) (`:180`)
  and [ADR 0443](../decisions/0443-agents-declared-as-durable-object-exports.md)
  Decision 3 assign to #539. The design notes' own promise is that rehydration catches
  "schema corruption or migration mismatches before application code runs"
  (`../bynk-design-notes.md`, storage section).
- **Posture:** Feature track per [ADR 0076](../decisions/0076-feature-track-posture.md),
  run GitHub-native per [ADR 0167](../decisions/0167-feature-tracks-run-github-native.md).
  It qualifies on all three axes (§2). It is the durable-data boundary: a wrong shape
  either loses user data silently or bricks live agents with no exit.
- **Evidence:** [#539](https://github.com/bynk-lang/bynk/issues/539)'s latest comment and
  [the 2026-10-01 language-implementation review](../reviews/2026-10-01-language-implementation-review.md)
  §3.1. One of their findings, that a class rename reuses the `[[migrations]] tag = "v1"`,
  is **superseded** by ADR 0443 (§3.5).

## What's already true

- **Additive evolution works.** ADR 0124 D4's zero-then-stored merge is built. A `store`
  field added after a record was written reads as its zero or initialiser.
- **Tightening faults.** ADR 0124 D1–D3's rehydration gate is built. A stored value that
  no longer satisfies the current type faults as an internal `RehydrationViolation`.
  There is no exit except a hand-staged migration through the agent's own handlers.
- **The stored shape is the wire shape.** [#1649](https://github.com/bynk-lang/bynk/issues/1649),
  the `loadState`/`commitState` codec disagreement that was this track's prerequisite,
  is closed. `commitState` writes `__encode<Agent>State(s)` (the boundary serialiser:
  `kind`-tagged sums, base64 `Bytes`, entries-array value `Map`s), and `loadState`
  decodes it through `__rehydrate<Agent>State` (`bynk-emit/src/emitter/emit.rs`,
  around `:5487`–`:5510`). A fingerprint describes this shape.
- **One key per agent instance.** Each agent instance stores its whole state record
  under the single Durable Object storage key `"state"`. No `store` kind gets its own
  keys.
- **Unknown stored keys are kept.** A stored key the current definition does not name
  (a renamed or removed field) is not decoded, and is written back on every commit. The
  emitter keeps it on purpose "so #539's migrations still find it" (`emit.rs`, around
  `:5508`). So today's rename loses the value from the program's point of view but not
  from storage: the old value is still there for a migration to recover.
- **`bynk.schema.lock` holds events only.** The format is `version = 1` plus an
  `[events]` table of `EventEntry { schema, fields: Vec<FieldShape> }`, where
  `FieldShape { name, ty, default }` (`bynk-project/src/schema_registry.rs`). It is
  reconciled in `bynk-check/src/schema_registry.rs`, and a non-additive change is
  refused with `bynk.event.non_additive_schema_change`. The file comes in and out of the
  compiler on its own channel (ADR 0321). Builds write it; `check` and `test` reconcile
  against it without writing (ADR 0493). A missing lock baselines silently (ADR 0299).
- **Agents are declared as `exports`.** Since ADR 0443 (#1796, #1803), each agent class
  is `[exports.<Class>] type = "durable-object" storage = "sqlite"`, and there is no
  `[[migrations]]` block or tag. The emitter never writes a tombstone. Removing an agent
  class fails the deploy (an orphaned namespace), which ADR 0443 keeps as a loud failure
  until this track gives removal a deliberate path. `exports` is one-way: a Worker
  deployed with it cannot go back to `[[migrations]]`.
- **Nothing else exists.** There is no agent-state fingerprint, no runtime schema
  version, no `migrate` declaration, no rename diagnostic, and no enumeration.
  `bynk migrate` is an unrecognised subcommand.

## 1. The theme

A deployed agent owns persisted state the maintainer did not write and cannot
regenerate. Foundations stability (Gate 1) keeps a 1.0 program's source compiling across
a 1.x upgrade. This track keeps its data loading. The two are different promises, and a
real upgrade needs both.

When this track retires:

- each agent class's stored state shape is recorded in `bynk.schema.lock`, so a build can
  see exactly how it changed;
- a non-additive change is caught at `bynk check` and refused unless the source says how
  to carry old data forward;
- a declared `migrate` transform runs at rehydration, before the ADR 0124 gate, and the
  gate still validates its output;
- renaming or deleting an agent class is a deliberate, declared act, not a Cloudflare
  deploy refusal;
- the driver can report, and where the platform allows, drive, the migration of a
  deployment's agents.

**The first fixture** is #539's reproduction:

```bynk
// before
agent Cart {
  key id: String
  store owner: Cell[String]
  ...
}

// after: checks and compiles with exit 0 today
agent Cart {
  key id: String
  store ownerName: Cell[String]
  ...
}
```

`ownerName` reads as `""`, and the old `owner` value rides along unread and is written
back on every commit. After S0 this is a `bynk check` error that names both fields.

## 2. Why a track (the ADR 0076 trigger)

- [x] **Multi-increment.** Five separable pieces (§4), each on a different layer: the
  lock and checker (S0), the emitted runtime (S1), the language surface (S2), the deploy
  config (S3), and the driver plus the Cloudflare API (S4). No single delete-on-merge
  proposal can carry them.
- [x] **Surface not yet settled.** Every question in §3 is open. The only nearby prior
  art in the tree is the events schema registry (ADRs 0299, 0321, 0493) and the unbuilt
  `via schema(...)` ranges (#990).
- [x] **Security/safety boundary.** This is the durable-data boundary (§6). The lock's
  agent-state format and any on-disk version marker are hard to reverse once real data
  has been written under them.

## 3. Open design questions

The settling agenda. Each names the investigation it needs. Marking this PR ready for
review asserts they are closed.

### 3.1 How is an agent's state shape recorded in the lock?

Options:

- **(a)** an `[agents]` table beside `[events]`, reusing `FieldShape { name, ty, default }`;
- **(b)** a new entry type that also records each field's storage kind
  (`Cell`/`Map`/`Set`/`Log`/`Cache`) and its refinement, since a kind change or a
  tightened refinement is a non-additive change too.

Sub-questions:

- Does `lock_version` bump from 1, and how does an old lock read?
- What is the key: the context-qualified class name?
- Does a missing lock still baseline silently, as for events?
- Does the lock record a version number per class (like `EventEntry::schema`), or only
  the shape, with the version derived from it?

*Needs:* `bynk-check/src/schema_registry.rs`'s `snapshot`/`reconcile_one`, and whether
`FieldShape::ty` already spells refinements and storage kinds distinctly.

### 3.2 Is a schema version written into Durable Object storage?

The lock records what the *source* expects. A deployed instance's stored record carries
no record of which shape wrote it. Without one, a `migrate` transform cannot tell an old
record from a new one that happens to look the same, and a deploy that skips an
intermediate version cannot chain transforms.

Options:

- **(a)** a version written beside `"state"`, under a reserved key or a reserved field
  inside the record;
- **(b)** no marker, and shape detection by inspecting the stored keys;
- **(c)** the fingerprint hash itself as the marker.

Sub-questions:

- What does an instance written before the marker existed read as (version 0, or "the
  first fingerprinted shape")?
- Is the marker written on every commit or only on migration?

*Needs:* the emitted `loadState`/`commitState` pair, and the `exports` storage backend
(SQLite) to confirm a second key is a separate write, not a second transaction.

### 3.3 What does the `migrate` surface look like?

ADR 0124 D5 asks for a "versioned-schema migration capability … a `via schema(...)` /
`@schema(N)` analogue that runs a declared transform on load", aligned with events.

Sub-questions:

- **Where is it declared?** On the agent, beside `store`, or as a top-level
  `migrate Cart from N { … }`?
- **What does it take as input?** A typed record of the previous fingerprint's shape. Is
  that type generated from the lock, or written out by hand in source?
- **Can it fail?** Is it total, or can it fail into `RehydrationViolation`?
- **Does it chain?** Do multiple versions compose (`1 → 2 → 3`), or is only the latest
  step kept?
- **Is it a handler?** If it is not, it needs no capabilities. Can it use `Clock` or
  `Random`?
- **Does default-on-read fall out of it?** ADR 0124 D5's per-field default-on-read, for
  narrowing, may be a one-field `migrate`, or it may need its own form.
- **What about rename and drop?** Is a pure rename (`owner` becomes `ownerName`) a
  `migrate`, or a lighter declaration (`store ownerName: Cell[String] was owner`)? Is
  dropping a field an explicit `drop owner`?

It must run before the ADR 0124 D1 gate, which then validates its output as untrusted
input.

*Needs:* the events track's `@schema(N)` and `via schema(...)` design
(`design/tracks/events.md`, #990), and a decision on whether the two share one mechanism.

### 3.4 How are renames detected?

With a fingerprint in the lock (§3.1), "a field removed plus a same-typed field added"
is visible **statically** at `bynk check`, not only at load.

Sub-questions:

- Is any non-additive state change an error until the source covers it with a
  `migrate`, rename or drop? Or is only the rename-shaped case an error and the rest
  warnings?
- Is the diagnostic's code `bynk.agent.non_additive_state_change`, the agent twin of
  `bynk.event.non_additive_schema_change`?
- Should rehydration also report an unknown stored key as a runtime backstop, for an
  instance deployed from a build that had no lock? It must not delete the key (see
  "What's already true").

The `owner → ownerName` reproduction (§1) is the first fixture, pinned as a lock-diff
diagnostic.

### 3.5 How are agent classes renamed and deleted?

The #539 comment and review §3.1 report that renaming a class re-emits
`[[migrations]] tag = "v1" new_classes = ["Basket"]` with the same tag. **That is
superseded**: ADR 0443 replaced the migrations block with declarative `exports`, so there
is no tag to reuse. Today a renamed class shows up as a removed class plus a new one.
Cloudflare refuses the removal (an orphaned namespace), so the deploy fails loudly, and
ADR 0443 Decision 3 reserves the tombstones (`state = "renamed"`/`"deleted"`) for this
track.

Sub-questions:

- **What declares it?** Which source declaration emits a tombstone, and does the lock's
  class key let `bynk check` see a class rename the way §3.4 sees a field rename?
- **How is deletion confirmed?** It destroys every instance's data. Is a source
  declaration enough, or does the deploy plan also need confirmation?
- **When can a tombstone be removed?** Is it permanent in the source?
- **How does it interact with the lock?** Does a renamed class carry its fingerprint
  history to the new name?

*Needs:* a check of Cloudflare's `exports` tombstone semantics at `WRANGLER_MIN`
(4.107.0) with `wrangler deploy --dry-run`, as ADR 0443's own verification did, and
whether a rename moves data between namespaces or only renames the binding.

### 3.6 Can the driver enumerate agents, and what is `bynk migrate`?

Agents are per-key Durable Objects with agent-local storage, so the language cannot list
them.

Sub-questions:

- **What can the platform list?** Does Cloudflare's API list the objects in a namespace?
  Does that include SQLite-backed objects and objects that have stored data, and does it
  return their names (agent keys) or only opaque IDs?
- **If it can list them:** an eager sweep is possible. `bynk migrate` would wake each
  instance through a reserved entry point, so it migrates and commits.
- **If it cannot:** migration is lazy (on rehydration only), and the verb means something
  else. It might be a dry-run report of what the next deploy changes, or a lock diff.
- **Is an eager sweep needed at all if S1 and S2 make lazy migration safe?** The case
  for it is retiring old `migrate` steps. A step can be deleted from the source only once
  no instance can still hold the old shape.

*Needs a platform check, not an answer from memory.*

### 3.7 Where does this sit against Gate 1?

Sub-questions:

- Is the `migrate` declaration part of the Foundations layer that 1.0 freezes (it sits on
  `store`, which is Foundations), or an advanced-layer addition that may grow through
  1.x?
- Either way, the lock's agent-state format and any on-disk marker are frozen at 1.0,
  because 1.x data must keep loading.
- Does the release discipline (`../bynk-release-discipline.md`) need a rule that a
  release changing the on-disk shape is a 2.0 event?

## 4. Candidate slice decomposition

Ordered so the loud failure lands before the mechanisms that resolve it. Provisional until
§3 settles. Each slice becomes an increment-proposal sub-issue of #2188 when it is cut.

- **S0: agent-state fingerprint in `bynk.schema.lock`, plus the non-additive-change
  diagnostic (§3.1, §3.4).** Builds, `check` and `test` reconcile agent state shapes as
  they do events. A removed, retyped or tightened field is refused. The `owner →
  ownerName` reproduction is the first fixture. This alone makes today's silent data
  loss loud, before any migration mechanism exists. Until S2, the way past the error is
  to widen instead of narrowing, or to stage the change through a new field by hand
  (ADR 0124 D3's convention).
- **S1: runtime schema version marker (§3.2).** Written on commit, and read on load
  before the D1 gate. An instance whose stored version has no path to the current one
  faults with a named cause, instead of reading a zeroed field.
- **S2: `migrate` at rehydration (§3.3).** A declared, typed old → new transform, plus
  rename and drop if §3.3 gives them their own forms. It covers S0's diagnostic for the
  change it describes. Its output still passes the ADR 0124 gate.
- **S3: agent-class rename and deletion (§3.5).** A source declaration emits the
  `exports` tombstone. Deletion is explicit and visible in the deploy plan.
- **S4: enumeration and the `bynk migrate` verb (§3.6).** Its shape depends on the
  platform check.

## 5. Front-loaded ADR candidates

- **The lock's agent-state entry format (§3.1).** Once projects commit locks with it,
  changing it is a lock-format migration of its own. It lands with S0.
- **The on-disk schema version marker (§3.2).** Once deployed agents have written it,
  its key and encoding are permanent. It lands with or before S1, and S0's lock format
  should be chosen knowing it.
- **The `migrate` surface's relation to events' `@schema(N)` (§3.3)**, if settling
  decides they share one mechanism. Otherwise this is an ordinary S2 ADR.

No ADR numbers are pre-allocated. They are assigned at merge.

## 6. Threat model

**Assets.** The persisted state of every deployed agent instance: user data the
maintainer did not write and cannot regenerate.

**Adversary.**

- **The main one is not hostile: a well-intentioned upgrade.** The developer renames a
  field, tightens a refinement, or renames or removes a class. Today the toolchain either
  accepts the change silently (the field rename: data unread and orphaned) or faults
  every instance with no exit (the tightening). The failure is silent at exactly the
  point the loud-fault philosophy (ADR 0124 D3) most needs it to be loud.
- **The second is ADR 0124's: tampered or corrupt stored bytes.** Rehydration already
  treats them as untrusted input. A `migrate` transform reads those same bytes, so it
  must not become a path around validation: its output goes through the D1 gate like
  any stored record.

**Where verification happens.**

- **At `bynk check`, `test` and build:** the lock diff (S0) catches a non-additive state
  change before deploy, on the same read-only path ADR 0493 gave events.
- **At rehydration:** the version marker (S1) and `migrate` (S2) run first, then the
  ADR 0124 D1 gate validates the result against the current type.
- **At deploy:** Cloudflare's refusal to orphan a namespace (ADR 0443 Decision 3) stays
  the backstop until S3 gives class removal a deliberate path.
- **Destructive acts are declared, never inferred.** Deleting a class or dropping a
  field always requires an explicit source declaration. Unknown stored keys are never
  deleted implicitly.

## 7. Slice status

- [ ] S0: agent-state fingerprint in the lock, plus the non-additive-change diagnostic
- [ ] S1: runtime schema version marker
- [ ] S2: `migrate` at rehydration
- [ ] S3: agent-class rename and deletion via `exports` tombstones
- [ ] S4: enumeration and `bynk migrate`

## 8. Done when

- [ ] Renaming, removing, retyping or tightening a `store` field is refused at
  `bynk check` unless the source covers it, and the `owner → ownerName` fixture pins
  this.
- [ ] A declared migration carries an old stored record forward on load, and the
  ADR 0124 gate validates its output.
- [ ] A deployed instance whose stored shape has no migration path faults with a named
  cause instead of reading zeroed fields.
- [ ] Renaming or deleting an agent class deploys through a declared path.
- [ ] The driver reports, and where the platform allows, drives, migration across a
  deployment's agents.
- [ ] The lock's agent-state format and any on-disk marker are recorded in ADRs and
  documented in the reference as part of the 1.0 promise.
