# State migrations: evolving a deployed agent's persisted state

- **Status:** Settling. Spine issue [#2188](https://github.com/bynk-lang/bynk/issues/2188)
  is open. Settling PR [#2189](https://github.com/bynk-lang/bynk/pull/2189) settles all
  seven design questions (§3). Nothing is built yet. One platform fact (§3.6, the
  SQLite object listing) is unverified, and it is S4's entry gate. Scopes design-review
  issue [#539](https://github.com/bynk-lang/bynk/issues/539) (schema fingerprint,
  `migrate` transform, rename detection, agent enumeration).
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
- [x] **Surface not yet settled (at settling time).** Every question in §3 was open when
  the track opened. The only nearby prior
  art in the tree is the events schema registry (ADRs 0299, 0321, 0493) and the unbuilt
  `via schema(...)` ranges (#990).
- [x] **Security/safety boundary.** This is the durable-data boundary (§6). The lock's
  agent-state format and any on-disk version marker are hard to reverse once real data
  has been written under them.

## 3. Design questions

All seven questions are settled below. Each records its decision, the evidence behind
it, and what it leaves to a slice. Only one fact is left unverified: whether Cloudflare's
object listing covers SQLite-backed namespaces (§3.6). It gates S4 only, not the
mechanism, and is recorded as S4's entry check rather than assumed.

### 3.1 How is an agent's state shape recorded in the lock? — SETTLED

**Decision.** The lock bumps to `version = 2` and gains an `[agents]` table keyed
`<unit>.<Agent>`, the same key scheme as `[events]`. Each entry records a per-class
integer `version`, a `floor` (§3.3), and a **deep** snapshot of the stored shape **for
every version** still at or above the floor. The snapshots of older versions are what
`migrate from N` steps are checked against (§3.3).

```toml
version = 2

[agents."shop.Cart"]
version = 2
floor = 1

[[agents."shop.Cart".shapes]]
version = 1
additive = false       # whether the transition *into* this version was additive

[[agents."shop.Cart".shapes.fields]]
name = "owner"
kind = "Cell"
type = "String"
default = false
refinement = ""        # canonical predicate text; empty when unrefined
# a record or sum type is expanded structurally, nested under `shape`

[[agents."shop.Cart".shapes]]
version = 2
additive = false

[[agents."shop.Cart".shapes.fields]]
name = "ownerName"
kind = "Cell"
type = "String"
default = false
refinement = ""
```

**When `version` bumps.** Every accepted change to the shape bumps the class's
`version`, additive or not, so that `__v` (§3.2) always names one exact shape. The new
snapshot records whether the change into it was additive.

**Why this shape.**

- **It records the storage kind.** `Cell[T]` changing to `Log[T]` changes the stored
  encoding. A `FieldShape` with only a type string would miss it.
- **It is deep.** The events snapshot (`canon_type`, `bynk-check/src/schema_registry.rs`)
  renders a user-named type as just its name, so a field renamed *inside* a stored record
  type is invisible to it. Stored state is persisted structurally, so the agent snapshot
  expands named records and sums down to the encoded shape.
- **It is a new function.** The agent snapshot is new code. The events snapshot is left
  as it is, because deepening it would make every committed events lock reconcile as
  changed.
- **It records refinements.** Each refinement is stored as canonical predicate text. The
  name-only spelling cannot see `type Email = String where Matches(A)` become
  `Matches(B)`.

**Why `version = 2` and not a silent addition.** `SchemaRegistry` has no
`deny_unknown_fields` (`bynk-project/src/schema_registry.rs:31`). Without a version
bump, an older Bynk would parse a lock that has an `[agents]` table, drop the table, and
write the lock back without it on its next build, silently losing every fingerprint.
`parse` already fails hard on a version it does not know, so `version = 2` makes an
older toolchain refuse instead.

**The upgrade path.**

- `parse` gains the one upgrade path that doesn't exist today: a v1 lock reads as v2
  with an empty `[agents]` table.
- A build writes it back as v2.
- `check` and `test` accept v1 and do not write it (ADR 0493).

**A missing lock, or a missing `[agents]` entry, baselines silently** at `version = 1`.
This matches events, and it is the only workable first build after upgrading to S0. The
drift this cannot see (a shape changed before S0 existed) is covered by the runtime
unknown-key report (§3.4).

**A lock key that disappears is an error.** Events drop a stale key silently. Agents
must not. An `[agents]` key with no matching class is `bynk.agent.class_removed`,
cleared only by the rename or removal declarations of §3.5.

### 3.2 Is a schema version written into Durable Object storage? — SETTLED

**Decision.** Option (a): a reserved field `__v` *inside* the stored record. It holds the
class's lock `version` and is written on every commit.

- **It is atomic.** The record and its marker are one value under `"state"`, one `put`.
  No crash can separate them, which meets the atomicity constraint from review.
- **The `__` prefix becomes reserved.** No check stops a store field from starting with
  `__` today. S0 adds `bynk.store.reserved_name`, which reserves the prefix for store
  field names, before any `__v` is ever written.
- **An absent `__v` means version 1.** Every instance written before S1 is read as the
  lock's baseline shape. That is correct, because S0 baselines the shape that was in
  source when it first ran.
- **The marker covers encoder changes, not just source changes.** The stored shape is
  `__encode<Agent>State`'s output, which is emitted TypeScript. If a compiler release
  changes the encoding of any storable type (a sum's tag, a `Map`'s entries layout),
  every stored record changes shape with no source change. Such a release must bump
  every affected class's `version` and ship the compiler-generated step that re-encodes
  it. That makes the encoding of each storable type part of the frozen data contract
  (§3.7), and it has a cost recorded there.

**Load order.**

1. Read `__v`. Call it `s`.
2. If `s` equals the current version, decode as today.
3. If `s` is below the current version, migrate it (§3.3 gives the exact rule).
4. If `s` is **above** the current version, the instance was written by a newer build:
   the deploy was rolled back. This faults as `RehydrationViolation` with cause
   `NewerSchema`. It never decodes the record and never writes it back, so a rollback
   cannot corrupt data written by the version it rolled back from.
5. Run the ADR 0124 D1 gate on the result.

### 3.3 What does the `migrate` surface look like? — SETTLED

**Decision.** One construct, declared inside the agent: `migrate from N`. Each step maps
stored version `N` **straight to the current shape**. Steps do not chain. The author
**declares the old shape**, and the lock **verifies** it.

```bynk
agent Cart {
  key id: String
  store ownerName: Cell[String]

  migrate from 1 (old: { owner: String }) {
    ownerName: old.owner
  }
}
```

**The rules.**

- **The parameter states version `N`'s shape.** The checker checks it against the lock's
  `[agents]` entry at version `N`, using the same deep comparison as §3.1. A parameter
  that doesn't match is `bynk.agent.migrate_shape_mismatch`.
- **The body is a record literal giving the new shape.** A field with the same name and
  shape in both versions carries over automatically and need not be written. Every other
  field must be assigned, or explicitly dropped with `drop owner`.
- **It is pure and total.** It has no `given` capabilities, no `Effect`, no `key` and no
  handler context. A sweep (§3.6) wakes an instance by its object ID, which doesn't
  recover the key, so a step that read `key` couldn't run there. Its output is still
  untrusted, so the ADR 0124 D1 gate validates it after the step runs. A value the step
  can't carry forward faults there, under the existing `RehydrationViolation`.
- **Steps map to the current shape, not to the next version.**
  - **Why not chain.** Chaining `from 1` into `from 2` would need the output type of
    `from 1`, version 2's shape, which is no longer in source. Typing it would mean
    building a type from the lock, the path rejected below.
  - **So each body is typed against the current state record**, which is always in
    source.
  - **The cost.** When version 3 lands, existing steps must produce version 3. Most
    still compile, because unchanged fields carry over by name. A step that no longer
    fits is an ordinary type error in its body, pointing at the line to revise.
  - **Old steps don't pile up.** The floor (below) retires them.
- **How a stored version `s` is migrated at load.**
  - If a `migrate from s` step exists, run it.
  - If every transition from `s` to current was additive, merge with zero (ADR 0124 D4).
    No step is needed.
  - Otherwise, fault as `RehydrationViolation` with cause `NoMigrationPath`.
- **What the checker requires.** Whenever a non-additive change lands, the version just
  before it needs a step (§3.4). For older versions, the checker warns with
  `bynk.agent.version_without_path` for each version between the floor and current
  that has no step and no additive path.
- **The floor retires steps.**
  - **What it is.** The `floor` in the lock is the oldest version any instance may
    still hold.
  - **What sets it.** Only a sweep (§3.6) raises it, after reporting zero instances
    below the new value.
  - **What it does.**
    - Snapshots below the floor are dropped from the lock.
    - `version_without_path` stops warning about them.
    - Steps below the floor are reported as deletable.
  - **Without a sweep** the floor stays where it is, and the warning stays honest about
    what might still be stored.

**Why declared, not synthesised.**

- **The alternative: synthesise it from the lock.** The compiler could build the type of
  `old` from the lock's deep snapshot. That needs a lock-to-type synthesis the checker
  has never had.
- **That leaves the old shape nowhere in source.** The decoder for version `N` would come
  from TOML that nobody reviews.
- **Declaring it has a precedent.** It follows ADR 0299's events precedent: `@schema(N)`
  is declared by the author and verified against the registry, not computed and hidden.
- **It keeps a self-contained record.** Old types the author has since deleted survive
  as inline record types in the step, so the migration stays readable after they are
  gone.
- **The cost is verbosity, only on breaking changes.** Additive changes need no step.

**What the emitter generates.** S2 generates a decoder for each step from the declared
`old` type, the same way `__rehydrate<Agent>State` is generated today, and a dispatch on
`__v` in `loadState`. It doesn't read the lock at emit time.

**Lighter forms are not added.** Renaming a field is a one-line step. There is no `was`
sugar and no rename annotation in this track. Sugar can be added in 1.x without breaking
anything. A second construct now would be one more thing for the Foundations promise to
freeze (§3.7).

**Default-on-read is a migration step.** ADR 0124 D5 deferred per-field default-on-read
for narrowing. Here it is just a step, for example
`count: if (old.count < 0) { 0 } else { old.count }`. Nothing separate is built.

**Not aligned with an events transform, because there isn't one.**

- **Breaking events change by convention.** A breaking event change introduces a new
  event type, and the registry refuses anything else.
- **Events have no upcast step yet.** Upcasting old events on read is deferred out of the
  events track (ADR 0287).
- **What is shared.** Agents and events share the lock file, `parse`/`serialize`, and the
  reconcile machinery. They do not share a surface.
- **Events can adopt it later.** If the events track wants a declared transform, it can
  adopt this one.
- **`@schema(N)` is not used on agents.** An agent's version is assigned by the compiler
  and verified by the steps.

**The soft rehydration handler stays deferred.** That is ADR 0124 D5's third item, a
typed recovery seam for corrupt data. It isn't needed for Gate 3: corruption still
faults, and evolution no longer needs to.

### 3.4 How are renames detected? — SETTLED

**Decision.** S0's diagnostic `bynk.agent.non_additive_state_change` is an **error**,
the agent twin of `bynk.event.non_additive_schema_change`.

**What it fires on.** Any of these, when no `migrate from <locked version>` step covers
the change:

- a removed field;
- a retyped field, including a structural change inside a stored record or sum;
- a changed storage kind;
- a field added with no default (impossible today for a `Cell`, which needs a zero);
- a field that lost its default.

**When a step covers it.** A step covers the transition when its declared shape matches
the lock and it assigns or drops every field that changed. The class's `version` then
bumps on the next build.

**Refinement changes are a warning, not an error.** `bynk.agent.refinement_changed`
fires when a predicate's canonical text changes under the same type. The compiler can't
tell widening from narrowing in general. Widening is safe, and narrowing is caught at
load by the ADR 0124 gate (D3). The warning says so and suggests a step if the change
narrows. If the author writes a step, it covers the change like any other.

**A pairing is never guessed.**

- **Exactly one candidate:** when exactly one removed field and one added field have the
  same kind and shape, the diagnostic's help suggests the rename step, for example
  `migrate from 1 (old: { owner: String }) { ownerName: old.owner }`.
- **Several candidates:** the diagnostic suggests nothing beyond "declare which field
  became which". It never applies a pairing itself (§6).

**The runtime backstop** is required, and it lands in S1 with the marker, since it is
the same load-path change. After migration runs, a stored key that is neither
a current field nor `__v` is reported once per load as a structured warning on the
agent's existing log channel. The key is never deleted. It is not a fault, because a
fault would brick every pre-S0 deployment that holds orphaned data. The report is how
pre-S0 drift becomes visible, and a step that drops the key is how it is cleaned up.

**The first fixture** is the `owner → ownerName` reproduction (§1). It is a negative
fixture at S0 and a positive one (the rename step) at S2.

### 3.5 How are agent classes renamed and deleted? — SETTLED

**Decision.** Two source declarations, each emitting one ADR 0443 `exports` tombstone.

```bynk
agent Basket replaces Cart { ... }   // [exports.Cart] state = "renamed", renamed_to = "Basket"
removed agent Wishlist               // [exports.Wishlist] state = "deleted"
```

**Rename (`replaces`).**

- **Cloudflare moves the data.** Per Cloudflare's class-exports reference: "Renaming a
  Durable Object class moves stored data from one class to another within the same
  Worker". Keys still resolve, because `idFromName(__serialiseAgentKey(key))` is
  unchanged.
- **The lock entry moves too.** It moves from `<unit>.Cart` to `<unit>.Basket`, keeping
  its `version` and step history.
- **A rename and a shape change can land together.** The shape change is reconciled
  against the moved entry, so it needs a step just as it would without the rename.
- **Bynk checks the target itself.** The checker requires the `replaces` target to have
  been in the lock and the new class to be live. A dry run of `wrangler deploy` at
  4.107.0 (`WRANGLER_MIN`) and at 4.149.0 accepted `renamed_to` pointing at a class that
  doesn't exist, so the toolchain can't rely on Wrangler to catch it.

**Delete (`removed agent`).**

- **It is permanent.** Per the same reference: "Deleting a class removes its namespace
  and all of its stored data permanently", with "no Trash".
- **A source declaration is not enough on its own.** The deploy plan prints
  `delete durable object Wishlist (all stored data; irreversible)`. `bynk deploy`
  refuses until it is confirmed: interactively, or with `--confirm-delete Wishlist` in
  CI. The confirmation names the class, so a stale flag can't approve a different
  deletion.
- **The build drops the lock entry.**

**Tombstone lifetime.**

- The tombstone is emitted for as long as the declaration stays in source.
- Cloudflare reports a tombstone that no longer changes anything with a `stale_tombstone`
  notice on deploy, and the docs say such entries are "safe to remove".
- `bynk deploy` passes that notice through in its output, naming the declaration to
  delete.
- Cloudflare also warns not to remove a tombstone while `referencing_scripts` is
  non-empty. Bynk's own cross-context calls don't bind another Worker's Durable Objects,
  so this applies only to hand-written bindings, and the deploy guide says so.

**Verified.** Wrangler 4.107.0 and 4.149.0 both accept a config with one `renamed` and
one `deleted` tombstone beside live `exports` entries (`deploy --dry-run`).

**Not verified.** A live rename against an account. Like ADR 0443's own checks, that
needs a Cloudflare account and is recorded on #2188 when S3 lands.

### 3.6 Can the driver enumerate agents, and what is `bynk migrate`? — SETTLED (S4 gated on a live check)

**Decision.** Migration is **lazy**: it runs at rehydration, as §3.2's load order sets
out. Lazy migration is complete for correctness on its own. Every instance is migrated
the next time it loads, and one that never loads never needs to be. An eager sweep
exists for one reason: deleting old `migrate from N` steps (§3.3) is only safe once no
instance can still be at version `N`.

**What the platform offers.**

- **The endpoint.** Cloudflare's API lists a namespace's objects:
  `GET /accounts/{account_id}/workers/durable_objects/namespaces/{id}/objects`, cursor
  paginated, up to 10,000 per page.
- **What each object carries.** Only an `id` and `hasStoredData`. There is no name, so
  agent keys can't be recovered.
- **Why that is enough.** A sweep doesn't need the key. A stub from `idFromString(id)`
  loads, migrates and commits the same record, and §3.3 makes steps key-free for this
  reason.
- **What the docs don't say.** They don't say whether the listing covers SQLite-backed
  namespaces, which are all Bynk emits.

**`bynk migrate` (S4) has two subcommands.**

- **`bynk migrate status`** works offline. It shows each class's lock version, its
  remaining steps, and what the next build would change.
- **`bynk migrate sweep [--context <c>] [--env <e>]`** lists each class's objects
  through the API, skips those without stored data, and wakes each of the rest through
  a reserved internal route. It then reports how many are at each version. When none
  is below some version, it raises the class's `floor` in the lock (§3.3), and the
  steps below it become deletable.

**The sweep route is a new authenticated surface (§6).**

- The route is `/_bynk/agent/__migrate` on the deployed Worker. The `/_bynk/` prefix is
  already reserved from user routes.
- **These constraints are settled:**
  - the route is authenticated;
  - the credential is controlled by the toolchain, not by the user's handlers or source;
  - the route reaches no handler;
  - the credential never appears in the deploy ledger's recorded state.
- **The mechanism is left to S4's proposal.** One option is to mint a short-lived
  credential for each sweep and rotate it. Another is a Cloudflare Access service token.
  The obvious "secret generated at deploy, recorded presence-only" design doesn't work
  on its own, because the CLI would never hold the value it must present.
- It does nothing but load and commit an instance. It returns that instance's `__v`
  before and after.

**S4's entry gate.** A live check that the objects listing returns SQLite-backed objects
with `hasStoredData` set. It is recorded on #2188 and run on an account, as #1809 did for
ADR 0443. If the listing doesn't cover them, `sweep` is dropped, `status` stays, and step
deletion is documented as unsafe until the platform allows it. Lazy migration is
unaffected either way.

### 3.7 Where does this sit against Gate 1? — SETTLED

**Decision.** The migration surface is **Foundations**.

- **The 1.0 definition puts storage in Foundations.** It lists "the core storage surface
  (`store` fields and their write forms)" (`../bynk-1.0-definition.md:51`–`55`).
- **So the migration declarations are Foundations too.** A Foundations program must be
  able to evolve the state it stores. `migrate from N`, `drop`, `replaces` and
  `removed agent` are therefore frozen at 1.0 with the rest of `store`.

**What Gate 3 freezes at 1.0, beyond the source surface.**

- The lock's v2 `[agents]` format.
- The `__v` marker: its name, its place inside the record, and what an absent value
  means.
- **The encoding of each storable type**, as `__encode<Agent>State` writes it.

**The carve-out this needs.** The 1.0 definition says emitted TypeScript "may improve
within a 1.x release". That stays true of everything except the stored wire shape. A 1.x
release may change an encoding only by shipping the compiler-generated re-encoding step
(§3.2). Otherwise the change is a 2.0 event.

**The cost, accepted (maintainer, in #2189's review).** A compiler that may re-encode
must keep a decoder for every encoding it has ever shipped, at least down to the oldest
floor (§3.3) any deployment might still hold. So legacy codecs are carried through the
whole 1.x line. The stricter rule was rejected: "no encoding change within 1.x; any
change is a 2.0 event". It would carry no legacy codecs, but a wrong 1.0 encoding would
then be stuck until 2.0. The stored-encoding ADR (§5) records the allowance and its cost.

This needs a one-line amendment to
`../bynk-1.0-definition.md`, beside the existing carve-out for the cross-context wire
format. That amendment is a front-loaded ADR (§5), and the same PR replaces the
definition's stale "(track to be opened)" at `:121` with a link to this doc.

**No change to the release discipline.** A milestone that bumps a class's encoding says
so in its cumulative migration note, which `../bynk-release-discipline.md` already
requires.

## 4. Slice decomposition

Ordered so the loud failure lands before the mechanisms that resolve it.

- **S0: agent-state fingerprint in the lock (§3.1, §3.4).**
  - Lock v2 and the v1 upgrade path.
  - The deep `[agents]` snapshot: a snapshot for each version, a version bump on every
    change, and the `floor`.
  - `bynk.agent.non_additive_state_change`, `bynk.agent.class_removed`,
    `bynk.agent.refinement_changed`, and the `__` reservation (`bynk.store.reserved_name`).
  - Fixtures: the `owner → ownerName` reproduction and the ambiguous same-type pairing.

  Until S2 lands, the only ways past the error are ADR 0124 D3's conventions: widen
  rather than narrow, or stage the change through a new field by hand.
- **S1: the `__v` marker and the unknown-key report (§3.2, §3.4).**
  - `__v` is written on commit.
  - Load faults with `NewerSchema` or `NoMigrationPath`.
  - The runtime unknown-key warning.

  No surface changes in this slice.
- **S2: `migrate from N` (§3.3).**
  - The parser and checker, including `bynk.agent.migrate_shape_mismatch`, `drop` and
    `bynk.agent.version_without_path`.
  - Step decoders and the `__v` dispatch in `loadState`: one step to the current shape,
    or a merge with zero.
  - The step clears S0's error for the change it covers.
  - The `owner → ownerName` fixture turns positive.
- **S3: class rename and deletion (§3.5).**
  - `replaces` and `removed agent`, emitting the tombstones.
  - Moving the lock entry with a rename.
  - The deletion confirmation in the deploy plan.
  - Passing through `stale_tombstone` notices.
- **S4: `bynk migrate` (§3.6).**
  - `status`.
  - `sweep` and its authenticated route, gated on the live listing check, which raises
    the `floor`.

## 5. Front-loaded ADR candidates

- **The lock's v2 agent entry (§3.1).** The deep snapshot, the key scheme, the version
  bump and its upgrade path. It lands with S0.
- **The `__v` marker and the frozen stored encoding (§3.2, §3.7).** The marker's place
  and meaning, rollback faulting with `NewerSchema`, and the rule that a 1.x encoding
  change ships its own step, with the legacy-codec cost that carries. It lands with S0,
  which reserves `__`, so that S1 builds on settled ground. It carries the one-line amendment to `bynk-1.0-definition.md`.
- **The `migrate` surface (§3.3).** Declared-and-verified, pure and total, with no lighter
  rename form. It lands with S2.

No ADR numbers are pre-allocated. They are assigned at merge.

## 6. Threat model

**Assets.**

- The persisted state of every deployed agent instance: user data the maintainer did not
  write and cannot regenerate.
- The account's compute budget, which the sweep route spends.

**Adversaries.**

- **A well-intentioned upgrade (the main one, and not hostile).** The developer renames a
  field, tightens a refinement, renames or removes a class, rolls a deploy back, or
  upgrades to a compiler whose encoder changed. Today the first of these is silent (data
  unread and orphaned), and the rest fault with no exit or are undefined.
- **Tampered or corrupt stored bytes (ADR 0124's adversary).** Rehydration already treats
  them as untrusted. A migration step reads those same bytes, so its output goes through
  the D1 gate like any stored record. A step cannot get around validation.
- **An outside caller of the sweep route (new in S4).** An unauthenticated route that
  wakes every instance would let anyone make the account pay for a full wake-up of
  every stored agent, and it would be a cost and denial-of-service tool.

**Where verification happens.**

- **At `bynk check`, `test` and build:** the lock diff (S0) catches a non-additive change
  before deploy, on the read-only path ADR 0493 gave events. A migration step's declared
  shape is checked against the lock (S2).
- **At rehydration:** `__v` dispatch, then a single step or the zero-merge, then the D1
  gate. A rollback faults
  with `NewerSchema` and is never written back.
- **At deploy:** class deletion requires a confirmation that names the class (S3).
  Cloudflare's refusal to orphan a namespace (ADR 0443 Decision 3) stays as the backstop
  for a class removed without a declaration.
- **At the sweep route:** a toolchain-controlled credential (the mechanism is S4's to
  settle, §3.6). The route only loads and commits, so it reaches no handler.

**Destructive acts are declared, never inferred.** Dropping a field (`drop`) and deleting
a class (`removed agent`) are explicit in source. Deletion is also confirmed at deploy.
Unknown stored keys are never deleted implicitly, and a field pairing is never guessed.

## 7. Slice status

- [ ] S0: lock v2 `[agents]` fingerprint, the non-additive-change diagnostics, and the `__` reservation
- [ ] S1: the `__v` marker, `NewerSchema`/`NoMigrationPath` faults, and the unknown-key report
- [ ] S2: `migrate from N` at rehydration
- [ ] S3: `replaces` and `removed agent` via `exports` tombstones
- [ ] S4: `bynk migrate status`/`sweep` (`sweep` gated on the live listing check)

## 8. Done when

- [ ] Renaming, removing, retyping or re-kinding a `store` field is refused at
  `bynk check` unless a `migrate` step covers it, a changed refinement warns, and the
  `owner → ownerName` fixture pins this.
- [ ] A declared migration carries an old stored record forward on load, and the
  ADR 0124 gate validates its output.
- [ ] A deployed instance whose stored shape has no migration path, or was written by a
  newer build (a rollback), faults with a named cause instead of reading zeroed fields,
  and is never written back.
- [ ] Renaming or deleting an agent class deploys through a declared path.
- [ ] The driver reports, and where the platform allows, drives, migration across a
  deployment's agents.
- [ ] The lock's v2 agent format, the `__v` marker and the frozen stored encoding are
  recorded in ADRs, `bynk-1.0-definition.md` carries the stored-encoding carve-out, and
  the reference documents all three as part of the 1.0 promise.
