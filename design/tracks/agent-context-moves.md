# Supported moves of agent-owning contexts between Workers

- **Status:** Settling. Spine bynk-lang/bynk#3565 (label `track`). It follows on from #3562
  (Worker grouping, #551) and its V0 verification
  ([#3562 comment](https://github.com/bynk-lang/bynk/issues/3562#issuecomment-6896203799)).
- **Ruling already given (maintainer, 27 November 2026, on #3565):** this goes into 1.0. The
  brief window of `1101` errors between phases 2 and 3 is **accepted and documented** in the
  first version; draining or queueing calls can come later. `bynk.deploy.agent_move_refused`
  stays exactly as it is until this lands.
- **Posture:** feature track per [ADR 0076](../decisions/0076-feature-track-posture.md), run
  GitHub-native per [ADR 0167](../decisions/0167-feature-tracks-run-github-native.md). It
  spans several increments (§9) and is a safety boundary: the refusal it replaces exists so
  that a deploy never silently starts an agent with empty state (§7.1). It changes no language
  surface: no grammar, no checker, no editor behaviour (hover, completion, semantic tokens and
  signature help are unchanged, because no source construct is added). It amends
  [ADR 0874](../decisions/0874-worker-groups.md)'s G5 and its "Future work" paragraph.
- **Not a build authorisation.** Each slice in §9 is cut as an ordinary increment proposal, a
  sub-issue of the spine.

Cloudflare facts carry their developers.cloudflare.com URL. Observed behaviour from the V0
probe is marked **(V0)**. Facts about Bynk come from `main` at `24c0475e` and carry a
`file:symbol` reference.

---

## 1. The problem, from the code

A context's agents are Durable Object classes, and a class's namespace (its stored data)
belongs to the Worker **script** that declares it. Worker grouping (#3562) made the script a
context runs on a deploy-time choice: `[deploy.workers.<key>]` puts several contexts on one
script. So changing a group changes the script of every context that joins or leaves it.

| # | Today | Where |
|---|---|---|
| P1 | **A context that hosts agents and would change script is refused** before anything is pushed, `--dry-run` included. A move is found by comparing the ledger's record of the script each context was last pushed under with the script it is now planned onto. | `bynk/src/deploy/agent_move.rs:agent_moves`, `:refusals`; called from `bynk/src/deploy/plan/run.rs:deploy` (`checked_agent_moves`) |
| P2 | **The one way through is to throw the state away.** `--accept-fresh-agent <context>` lets the move go ahead with fresh, empty namespaces on the new script; the ledger records that the old script still holds the data, so the orphan report says "do not delete". | `agent_move.rs:record_fresh_moves`, `:fresh_warning`; `bynk/src/deploy/ledger.rs:DeployLock::record_fresh_agents`, `WorkerRecord::fresh_agents`, `find_orphans` (`worker_agent_state`) |
| P3 | **Two shapes cannot be done at all,** not even fresh: a context leaving a group that keeps running (`reason = "old_script_stays"`: "Cloudflare cannot drop a class from a Worker that stays") and a group renamed in place (`renamed_in_place`). | `agent_move.rs:refusals` |
| P4 | **The guard exists because a bare re-declare is silent.** Declaring the class in a second script, without a transfer, makes a new empty namespace and reports success (V0 step 2). Bynk's emitter declares every class as a plain live `exports` entry. | `bynk-emit/src/emitter/wrangler.rs` (the `[exports.<Class>]` loop, `push_tombstones`) |
| P5 | **The exports map can express the move, but Bynk never writes it.** The emitter writes only `created`, `renamed` and `deleted`; nothing writes `expecting-transfer` or `transferred`. | `bynk-emit/src/emitter/wrangler.rs:push_tombstones`, `ClassTombstones` |
| P6 | **A deploy records only what it saw succeed, Worker by Worker.** The ledger is written after each Worker's push; an interrupted push is not recorded, because whether it landed is unknown. There is no record of a step *inside* a Worker's deploy, and none of a multi-push operation. | `bynk/src/deploy/apply.rs:apply_plan` (`Pushed::Interrupted`), `:record_pushed`, `LedgerWriter` |
| P7 | **The deploy facts cannot say "mid-move".** `BYNK_DEPLOY_FACTS` lists the classes a version declared and, for a group, its members; revert refuses when either differs (`bynk.revert.class_changed`, `bynk.revert.membership_changed`). | `bynk/src/deploy/facts.rs:Facts`; `bynk/src/deploy/revert.rs:evaluate`, `membership_finding` |
| P8 | **There is no lock between deploys.** Two runs from two checkouts are not serialised. The only optimistic check is revert's: it re-reads a Worker's deployments just before the rollback and stops if anything was deployed since the plan. | `revert.rs:execute` (`Stopped::Moved`) |

The user-facing consequence: an already-deployed project whose contexts host agents cannot
adopt Worker grouping without losing its agents' data, which is why the ruling puts this
into 1.0.

**What already exists to build on:**

- the move detection, its JSON-plan entries and the flag-per-context pattern (`agent_move.rs`,
  #4517: `refusals` and `warnings` entries with an `override`);
- the ledger's incremental, resumable posture (ADR 0180, `apply.rs:apply_plan`'s comment),
  the atomic `write_lock` and the additive, `default`ed field convention used by every field
  added since v1 (`ledger.rs:Environment`, `WorkerRecord`);
- the `exports` tombstone emission (`wrangler.rs:push_tombstones`) and the per-push record of
  carried tombstones (`ledger.rs:Environment::deleted_classes`, `renamed_classes`);
- the account's Durable Object namespace listing, `(id, script, class)`, already used by the
  sweep and revert (`bynk/src/migrate/sweep.rs:Cloudflare::namespaces`, `Namespace`);
- the Worker deployments listing, which stays on wrangler (`WorkerDeployments`,
  [ADR 1075](../decisions/1075-account-resources-need-api-credentials.md)); the push and
  `rollback` stay on wrangler too, so this needs no new REST surface after #4056.

## 2. What Cloudflare provides

From [Durable Object class exports](https://developers.cloudflare.com/durable-objects/reference/durable-objects-migrations/):

- The states are `created`, `deleted`, `renamed` (`renamed_to`), `transferred`
  (`transferred_to`) and `expecting-transfer` (`storage`, `transfer_from`). "The class name
  remains the same; the value tells Cloudflare what to do with the existing namespace."
- A transfer is four deploys: the source as it is; the **target** adds `expecting-transfer`
  with `transfer_from` and **no** Durable Object binding; the **source** changes its entry to
  `transferred` with `transferred_to`, **keeping the class in code**, and that deploy
  "atomically reassigns the namespace to the target Worker"; the **target** then adds the
  binding and redeploys "once the source rollout is complete".
- A pending transfer is cancelled by the target removing its `expecting-transfer` entry.
- Durable Object entries in `exports` "are mutually exclusive with `migrations`", and a Worker
  deployed with `exports` cannot go back to `migrations`.
- "Rollbacks cannot cross a lifecycle change", and `wrangler versions upload` does not apply
  lifecycle changes, so every phase must be a `wrangler deploy`.
- "Each environment maintains its own provisioned namespaces", and "both Workers must live in
  the same Cloudflare account".
- An error, `phase_one_transfer_target_class_provisioned`, is raised when "the target Worker
  already owns a namespace for the class".
- The page says nothing about request errors during a transfer, about a second transfer of
  the same class, or about transferring a class back.

Observed in V0 and not in the docs: phase 1 with a binding fails with `100406`; requests
through the target between the source's commit and the target's cleanup deploy fail with
`1101`; a target redeployed unchanged after the commit fails with
`phase_one_transfer_after_commit_mismatch`; the V0 source **dropped** the class from its code
in phase 2 and the transfer still committed. The docs say to keep it in code, so this design
follows the docs, and the live check (§10, S4) tries both.

## 3. The orchestration in `bynk deploy`

### 3.1 Terms

A **move** is one context `c` with agent classes `K`, going from Worker `Wf` on script `Sf`
to Worker `Wt` on script `St`, in one environment (scripts are environment-qualified,
`bynk_project::environment::deployed_worker_name`). A **move set** is the connected set of a
run's moves: all the moves that share a source or a receiver. So two standalone Workers `A`
and `B` that both host agents and join group `G` make one set with **two sources and one
receiver** (fan-in), and a group split into two Workers makes one set with one source and two
receivers (fan-out). A set is moved as one unit, with **one push per Worker per phase**. A
receiver's phase 1 push carries one `expecting-transfer` entry per incoming class, each with
its own `transfer_from`, and a source's phase 2 push carries one `transferred` entry per
outgoing class, each with its own `transferred_to`. Whether one push may carry several
sources' entries is live check S13. A Worker that is both a source and a receiver in one run
is refused (`chained`, D9).

### 3.2 The phases

Each phase is one ordinary Worker push per Worker of the set, through the existing
`provisioning.rs:push` (`wrangler deploy`). What it pushes is a **phase variant** of the
Worker's generated `wrangler.toml` (and, for phase 1, of its entry), which the emitter writes
beside the normal one (§9, slice 2). The normal provision → secrets → push steps of
`apply.rs:apply_worker` run as for any push; only the build differs.

| Phase | Workers | What each pushes | Effect |
|---|---|---|---|
| **1, receive** | each receiver `Wt` (`St`) | `Wt`'s new build with each incoming `c` as a **forwarded member**: the receiver's dispatcher hands every request that `c` would own (its routes, and `/_bynk/call/<service>` and `/_bynk/event/<service>` for `c`'s services) unchanged to `c` on `Sf` over a Service Binding, and runs none of `c`'s code itself. The same build also carries the class code of `K` exported, each class as `state = "expecting-transfer"`, `storage = "sqlite"`, `transfer_from = "<Sf>"`, and **no** Durable Object binding for it. A brand-new `St` is a first deploy. | `St` expects the classes and answers for `c` by forwarding. Nothing has moved; the data is still `Sf`'s, and every call to `c`, wherever it arrives, is served by `c` on `Sf`. |
| **2, commit** | each source `Wf` (`Sf`) | `Wf`'s new build (or, when `Wf` is no longer a Worker of the build, a **stub**: a `fetch` that answers `503`), with each outgoing class still exported in code, its binding removed, and `state = "transferred"`, `transferred_to = "<St>"`. | Cloudflare reassigns the namespaces to `St`. From here the data is `St`'s. **The window opens** with the first of these pushes. |
| **3, go live** | each receiver `Wt` (`St`) | `Wt`'s normal new build: `c` a member, each class plain live, with its binding. | `St` serves `K` with the old data. **The window closes** with the last of these pushes. |

Because phase 1 forwards `c` rather than running it, it opens no window. A member of `St`'s
group that calls `c`, and a caller already rebound to `St`, both reach `c` on `Sf` until
phase 2, exactly as before the run. A forwarded request is passed on verbatim, signature and
body included, so `c` on `Sf` verifies it as it does today. That needs each edge key of `c`'s
callers to keep its value when the caller is rebound from `Sf` to `St` (slice 4 tests it).
The only window is the one the ruling accepted, from phase 2 to phase 3.

**Placement in the run's order** (`gates.rs:ordered`, callee first). The set is ordered as
**two nodes**. *Receive* (every phase 1 push) depends on every Worker that the receivers call
outside the set. *Commit* (every phase 2 push, then every phase 3 push, back to back with no
prompt and nothing else between them) depends on *receive*, on every Worker that the sources
call outside the set, and on **every Worker that calls a moved context**. So those callers
are pushed between the two nodes, already bound to `St`, while `St` still forwards to `Sf`.
If the order cannot be built (a cycle through the two nodes), the run is refused
(`reason = "order"`). Each Worker of the set is pushed only by its phases, not again in its
ordinary slot, and the rest of the order continues after *commit*. So no caller is still
bound to `Sf` when phase 2 lands, and the window is phase 2 to phase 3 for every caller.

### 3.3 What is recorded, before and after each phase

The ledger gains one record per open move set (§4). Each line below is one `write_lock`,
through the same `LedgerWriter` the run already uses, so each is atomic and durable. Within a
phase, each Worker's push is recorded when it is done (`done`), because a set's phase can be
several pushes.

| When | Ledger: the set's `step` | Also recorded |
|---|---|---|
| Before phase 1 | `receiving` | a fresh random `move` id; the moves (`context → classes`, `from`, `to`); `started`; the bynk version; the **live deployment id of every Worker of the set**, as read just before the push (`none` for a brand-new `St`); the ledger `version` becomes 2 (D5) |
| After each receiver's phase 1 push | `receiving` | that receiver in `done`, with its new deployment id |
| All receivers done | `received` | `done` cleared |
| Before phase 2 | `committing` | each source's live deployment id as read just before its push |
| After each source's phase 2 push | `committing` | that source in `done`, with its new deployment id; its Worker record as for any push (its members no longer list the contexts it gave up; its `transferred` tombstones in a new `transferred_classes` map beside `deleted_classes`) |
| All sources done | `committed` | `done` cleared |
| Before phase 3 | `going_live` | nothing more |
| After each receiver's phase 3 push | `going_live` | that receiver in `done`; its Worker record as for any push (`contexts`, `script`, contracts) |
| All receivers done | record **removed** | the ledger `version` returns to 1 when no record is open |

A push that fails leaves the record as it was and the run stops, as any failed push does
today (`apply.rs:apply_plan`, `stopped_report`). An interrupted push (`Pushed::Interrupted`) is
treated the same way: not recorded as done.

### 3.4 What a re-run does from each recorded step

A re-run reads the open record and **resumes the same set** without `--move-agent` being
passed again (the record is the consent already given), provided the manifest still asks for
the same moves. Before each push it reads the account (D7): the namespace listing for each
class of `K` (`sweep.rs:Cloudflare::namespaces`, which reports a namespace's `script`), and the
deployments of every Worker of the set, with the facts of each one's newest version.

**Own push or someone else's.** Every phase push carries the set's `move` id and its phase in
its deploy facts (`transfer.move`, `transfer.phase`, §4.2). A deployment of a Worker of the
set that is newer than the id the record holds for it is **this move's own push** only if its
version's facts carry this `move` id and the phase the record says was under way for that
Worker. Then it is the push that landed but was never recorded: the run records it as done
and does not push it again. Any other newer deployment (no facts, another `move` id, another
phase, or a version with no `transfer` where one was expected) was made by something else,
and the run stops with `bynk.deploy.move_interleaved` (§7.2). Phase 3 versions also carry the
`move` id while the record is open, so the same test covers them.

| Recorded step | What the account shows, per Worker not yet `done` | The re-run |
|---|---|---|
| `receiving` | no deployment newer than the recorded id | pushes its phase 1 |
| `receiving` | a newer deployment that is this move's own phase 1 | records it `done` |
| `received`, `committing` | `K`'s namespaces still on `Sf`, and no newer deployment of `Sf` | pushes its phase 2 (S3 gates this row for a source whose phase 2 may already have been attempted) |
| `committing` | a newer deployment of `Sf` that is this move's own phase 2, and `K`'s namespaces on `St` | records it `done` |
| `committed`, `going_live` | `K`'s namespaces on `St` | pushes its phase 3, or records it `done` if its newest deployment is this move's own phase 3 |
| `committed`, `going_live` | `K`'s namespaces on `Sf`, or on neither | **stops**: `bynk.deploy.move_state_unknown`, naming the classes and both scripts. Never declares `K` live on `St` (§7.1) |
| any | a newer deployment that is not this move's own | **stops**: `bynk.deploy.move_interleaved` |
| any | the manifest no longer asks for these moves | **stops**: `bynk.deploy.move_open` (D8) |

**Resuming only goes forward.** Cloudflare's docs say "rollbacks cannot cross a lifecycle
change" ([exports](https://developers.cloudflare.com/durable-objects/reference/durable-objects-migrations/)),
and every phase is one. So no step of the orchestration, and no recovery from a failed or
interrupted phase, uses `wrangler rollback`: a failed phase is repeated or the move goes on to
the next one. Before any phase 2 push has landed, the one way back is Cloudflare's documented
cancel (D8), which is itself a forward deploy of each receiver. From the first landed phase 2
push on, there is no way back except a second, forward move (§6). This is also why
`bynk revert` refuses across a move.

**Resuming is idempotent.** Before each push, the re-read asks whether that push already took
effect (its own deployment is there, by the test above). If it did, the push is recorded as
done and not repeated. When the re-read before a source's phase 2 finds `K`'s namespaces
already on `St` and that source's newest deployment is this move's phase 2, the source is
recorded `done` and not pushed again. The guarantee is that each phase **takes effect** at
most once per Worker. It is not that each phase is *pushed* at most once: a push that failed
before it took effect is pushed again (the first two rows of each step), and live check S3
must show that repeating phases 1 and 2 is safe before those rows are built. Phase 3 is a
plain redeploy, so repeating it is always safe.

The namespace listing is the single source of truth for "who holds the data", and the ledger
only says where the run had got to. This keeps ADR 0194 D1's rule that the ledger records
what was pushed and never claims to know the namespaces (`ledger.rs:Environment::deleted_classes`'s
comment; the "Understand" box in the deploy guide's *Durable Objects* section). Whether the
listing shows the reassignment at phase 2, and whether it shows an `expecting-transfer`
namespace at all, is live check S5. If it does not, the resume relies on the deploy facts'
`move` and `phase` alone, which the test above already uses.

## 4. The ledger and `BYNK_DEPLOY_FACTS`

### 4.1 The ledger

A new, additive table under each environment (`ledger.rs:Environment`), `default`ed and
omitted when empty, like every field added since v1:

```toml
[[environments.default.agent_moves]]
move = "3f9c2a71"             # random; also carried in each phase version's facts
step = "committing"           # receiving | received | committing | committed | going_live
started = "2026-11-27T10:15:02Z"
bynk = "…"                    # the bynk version that started it
done = ["shop-orders"]        # Workers whose push for the current step has landed

[[environments.default.agent_moves.moves]]
context = "shop.orders"
classes = ["Cart"]
from_worker = "shop-orders"
from_script = "shop-orders"
to_worker = "core"
to_script = "core"

[[environments.default.agent_moves.moves]]
context = "shop.billing"
classes = ["Ledger"]
from_worker = "shop-billing"
from_script = "shop-billing"
to_worker = "core"
to_script = "core"

[environments.default.agent_moves.deployments]   # each Worker's last-seen deployment id
shop-orders = "…"
shop-billing = "…"
core = "…"                    # "none" for a receiver that did not exist
```

This example is a fan-in: two standalone Workers joining group `core`, part-way through
phase 2.

Script names are stored as the ledger already stores them (`WorkerRecord::script`), and the
environment-qualified name is derived as it is today. The record is a record of steps, not of
namespaces.

**Versioning.** `read_lock` refuses any `version` other than 1 (`ledger.rs:read_lock`,
`lock_version`), and no ledger struct denies unknown fields, so a bynk from before this
feature would **silently ignore** an `agent_moves` table and plan the half-done move as if it
had not started. After phase 2 that bynk would push `Sf` with `K` as plain live classes. So
the ledger is written as `version = 2` **while any move is open**, and as 1 otherwise (D5).
An older bynk then stops on "unsupported deploy lock version 2", which is what should happen
mid-move. This bynk reads both.

### 4.2 `BYNK_DEPLOY_FACTS`

`Facts` (`facts.rs`) gains one additive, optional field, and `FACTS_VERSION` stays 1:

```json
{"v":1, "classes":["Other"], "transfer":{"move":"3f9c2a71","phase":1,"expecting":{"Cart":"shop-orders"}}, "contexts":["shop.pay"]}
```

- Every phase version carries `transfer.move` (the set's id) and `transfer.phase` (1, 2 or 3),
  which is how a resume tells its own unrecorded push from anything else (§3.4).
- A phase 1 version of `St` carries `transfer.expecting` (class → source script), does **not**
  list `K` in `classes` (it declares them pending, not live), and lists the members it serves,
  which do not yet include `c`.
- A phase 2 version of `Sf` carries `transfer.transferred` (class → receiver script) and does
  not list `K` in `classes`.
- A phase 3 version carries `transfer.move` and `transfer.phase = 3` only, and lists `K` in
  `classes`. The next ordinary push writes no `transfer` at all.

Keeping `K` out of `classes` while it is not live means a bynk that does not know the
`transfer` field (it decodes it as absent, since `Facts` does not deny unknown fields) still
refuses to revert across a move by the existing `bynk.revert.class_changed`. Bumping
`FACTS_VERSION` was rejected: `Facts::decode` reads any other version as "no facts", which
would make every version written by the new bynk unrevertible to an older one. The field is
well under a hundred bytes, inside `MAX_FACTS_BYTES`.

### 4.3 Stored-state encoding

The move does not touch the stored-state encoding that `design/bynk-1.0-definition.md` freezes
at 1.0 (ADR 0538): Cloudflare reassigns the namespace and no record is read or rewritten by
Bynk. The class's schema version (`Facts::agents`) and storage layout (`Facts::layout`) go
with the class to `St`, the schema lock is keyed by `<context>.<Agent>` and does not change,
and `bynk migrate`'s per-Worker sweep follows the class through `migrate::worker_of`. The
ledger and the facts are tooling surface, versioned with the toolchain, and both changes are
additive except the temporary `version = 2`, which is there to stop an older reader.

## 5. The error window

**Accepted** (ruling). Between the first phase 2 push landing and the last phase 3 push landing,
a request for `c` fails. One that reaches `K` on `St` fails with `1101` (observed in V0), and
one that `St` still forwards reaches `Sf` without `c` (its router's 404) or the stub (503).
For a set with one source and one receiver the window is two pushes long at best, the end of
phase 2's push plus the whole of phase 3's push. It is one push longer for each further
source or receiver, and Cloudflare's
docs add that phase 3 should follow "once the source rollout is complete", which is also
propagation time. The live check (S1) measures it; until then the docs give no number.

**What a caller sees:**

| Caller | During the window | After |
|---|---|---|
| A route or custom domain on `St` that reaches `K` | `500` (Cloudflare error `1101`) | normal |
| Another member of `St`'s group calling `c` in-process | the callee's throw, answered as the plain `500` a Service Binding gives, which a Bynk caller receives as a callee fault (ADR 0874, G3-2) | normal |
| A Worker that calls `c` (rebound to `St` before phase 2, §3.2) | `St` forwards to `Sf`, which no longer has `c`: `404` from `Sf`'s router, or `503` from the stub | normal |
| A route or custom domain still on `Sf` (D12) | as above | moved to `St` in phase 3, or refused |
| Anything that does not touch `c` | unaffected | — |

Every caller is rebound before phase 2, so no caller is in the window for longer than phase 2
to phase 3. The plan lists them.

**Where the docs say it:**

- the plan: `bynk deploy` (and `--dry-run`) prints a move section naming each move set, its
  classes, the phases, the callers rebound after it, and the window as a warning
  (`bynk.deploy.agent_move_window`), which the JSON plan carries as a `warnings` entry
  (#4517's shape) beside a `moves` array;
- `book/guides/projects-build-and-deployment/deploy-to-cloudflare.md`: a new *Moving a context
  that hosts agents* section under *Multi-context projects*, with the phases, the measured
  window, the caller table above, and what a failure part-way means (beside *When a deploy
  fails part-way*);
- `reference/tooling/manifest.md#worker-groups`: the paragraph that today says the move is
  refused;
- `reference/tooling/bynk-cli.md`: the `--move-agent` row, the new codes in the JSON plan
  table, and the `override` change on `bynk.deploy.agent_move_refused`;
- `book/guides/projects-build-and-deployment/roll-back-a-deployment.md`: the two new revert
  refusals;
- `bynk explain` entries for each new error code.

Draining or queueing calls during the window is out of scope (ruling) and is left as a
later improvement; nothing here closes the door on it (a drain would sit between phases 1 and
2, where nothing has moved yet).

## 6. `bynk revert` across a move

Cloudflare's own rule is that "rollbacks cannot cross a lifecycle change"
([exports](https://developers.cloudflare.com/durable-objects/reference/durable-objects-migrations/)).
Bynk refuses before it asks, with a reason:

| State | `bynk revert` of `St` or `Sf` |
|---|---|
| A move is open (any `step`) | **refused**, `bynk.revert.move_open`: finish the move with `bynk deploy` first. Other Workers of the project are judged as usual. |
| Closed; the target is a version from before the move | refused by the existing `bynk.revert.class_changed` (the classes differ) and, for a group, `bynk.revert.membership_changed` (`revert.rs:evaluate`, `membership_finding`) |
| Closed; the target is a phase 1 or phase 2 version | **refused**, `bynk.revert.move_version`: a version from the middle of a move (its facts carry `transfer`); also caught by `class_changed`, but named for what it is |
| Closed; the target is after the move (both versions carry `K` live, same members) | judged as today |

**Undoing a move is a forward move back**, with `--move-agent` the other way. A transfer back
is unprobed (§2); if S7 shows it does not work, a move whose receiver is a script the class
was transferred off is refused (`reason = "transferred_back"`) until it does, and the docs say
so.

## 7. Threat and failure model

### 7.1 An empty class silently created

The reason the refusal exists (P4). The invariants:

1. A class of `K` is declared plain live on `St` **only** in phase 3, and phase 3 runs only
   from `committed`/`going_live`, after every source's phase 2 has landed and the account shows `K`'s namespaces on `St` (§3.4).
2. A move that is not named (`--move-agent`) and has no open record is refused exactly as
   today (`bynk.deploy.agent_move_refused`); nothing in this design weakens the gate for an
   unnamed move.
3. `--move-agent c` and `--accept-fresh-agent c` together are refused (`reason = "both"`).
4. The receiver must not already own a namespace for a class of `K` (Cloudflare's
   `phase_one_transfer_target_class_provisioned`); the plan checks the namespace listing
   first and refuses (`reason = "receiver_has_class"`), which covers a receiver that was
   started fresh by an earlier `--accept-fresh-agent`.
5. Phase 3 attempted without a commit (S6 asks what Cloudflare does) can never be reached by
   the orchestration; the check is there in case of a bug, not as a path.

### 7.2 Concurrent deploys

There is no lock today (P8), and this design does not add one. The guards:

- the `version = 2` ledger stops an older bynk reading a ledger mid-move;
- every Worker of the set has a deployment-id baseline from before phase 1, and before each
  push the run re-reads the deployments. A newer deployment that is not this move's own (its
  facts lack this `move` id and the expected phase, §3.4) stops the run
  (`bynk.deploy.move_interleaved`), in the style of revert's `Stopped::Moved`;
- a second checkout without the open record (the ledger not yet committed) plans the move
  afresh. Its pre-phase-1 read finds `K` already on `St`, or a receiver whose newest version
  carries another `move` id, and it stops with "the move is already under way or done: pull
  `bynk.deploy.lock`".

Two runs racing inside the same few seconds can still both pass a check. The docs say what
they say today about CI: one deploy at a time per environment, and commit the ledger.

### 7.3 A deploy interrupted mid-move

Covered by §3.3–3.4. The failure that matters is between phase 2 and phase 3, when the data
is `St`'s and `St` is not serving it: the run stops with a message that says the move is
committed and a re-run will finish it, and the next `bynk deploy` does phase 3 first. No
state is lost by any failure, because before phase 2 the data is still `Sf`'s and live there,
and after it Cloudflare holds it on `St`.

### 7.4 Out of scope

Moves across accounts (Cloudflare does not support them) and across environments (each has
its own namespaces). A move combined with a schema change of the same class is allowed: the
schema migration runs per instance, on read, wherever the class lives.

## 8. Decisions

### D1. The move is opt-in, per context — **Maintainer ruling needed**

**[DECISION]** Recommend `--move-agent <context>`, repeatable and never a blanket, as ADR 0874's
future-work paragraph sketched; without it a move stays refused. The refusal's `override`
becomes `--move-agent <context>` and its message also names `--accept-fresh-agent` for
discarding the state. A `--move-agent` name that matches no move is an error
(`bynk.deploy.move_agent_unused`), as for the fresh flag.
*Rejected:* moving automatically whenever a context changes script (one manifest edit would
start a run with an outage in it, unannounced); `--yes` implying it (`--yes` only skips
confirmation, nowhere a refusal). This is a product call because it decides whether adopting
groups is one step or two for every user with agents.

### D2. The phases run as two nodes of the order, with callers rebound between them

**[DECISION]** Recommend §3.2. Phase 1 pushes each receiver with the moved contexts as
forwarded members. Then every caller of a moved context is pushed, rebound to the receiver.
Then phases 2 and 3 run back to back. So the only window is phase 2 to phase 3, for every
caller, as the ruling accepted.
*Rejected:* phase 1 pushing the receiver's plain new build (the receiver's own members, and
any caller rebound to it, would call `c` there without its binding from phase 1 onward,
which is earlier than the accepted window); rebinding callers after phase 3 (each caller
would get `Sf`'s 404 from phase 2 until its own push, longer than the window); pushing phase 1
as the receiver's *previous* build (Bynk keeps no build trees of past deploys, and a CI
checkout has none); one run per phase (two runs with an outage open between them); a
separate `bynk move` command (the move is a consequence of the manifest the deploy already
plans).

### D3. The phases are generated config variants pushed by the existing push

**[DECISION]** Recommend the emitter writes `wrangler.receive.toml` and `wrangler.commit.toml`
(names to settle in slice 2) beside the Worker's normal config, and a stub Worker for a
dissolved source, and the push takes the config path. *Rejected:* editing the generated
`wrangler.toml` in place at deploy time (the build tree stops describing what was built); a
REST upload (ADR 1075 keeps the push on wrangler, and `wrangler versions upload` cannot carry
lifecycle changes).

### D4. The ledger records the move set and its step, before and after each push

**[DECISION]** Recommend the additive `agent_moves` table, the five steps of §3.3, the per-Worker `done` list, a deployment-id baseline for every Worker taken before phase 1, and a `move` id carried in each phase version's facts.
*Rejected:* recording only completed phases (an interrupted phase 2 would be invisible, and
the re-run could not tell "not pushed" from "pushed but not recorded" without trusting the
account alone); recording the namespaces themselves (ADR 0194 D1).

### D5. `version = 2` while a move is open

**[DECISION]** Recommend it, so an older bynk refuses a ledger mid-move instead of silently
ignoring the record (§4.1). *Rejected:* a permanent bump to 2 (it forces every user on to the
new bynk for a field most never write); leaving it at 1 (an older bynk could push `Sf` with
the classes live after the commit).

### D6. Facts: an additive `transfer` field, `K` out of `classes` while not live

**[DECISION]** Recommend §4.2. *Rejected:* `FACTS_VERSION = 2` (every new version becomes "no
facts" to an older revert).

### D7. The account is read before each phase; no lock

**[DECISION]** Recommend reading the namespace listing and the deployments before each phase,
and recording deployment ids (§3.4, §7.2). *Rejected:* an account-side lock (none exists for
deploys; a lock Worker or KV key is new infrastructure on the user's account for one
feature); trusting the ledger alone (a second checkout's ledger is not this one's).

### D8. Resuming needs no flag; a changed manifest while a move is open is refused

**[DECISION]** Recommend that an open record resumes on the next plain `bynk deploy`, and that
a manifest which no longer asks for the move is refused (`bynk.deploy.move_open`). Before
any phase 2 push has landed, the user may cancel with `--cancel-move <context>`. It names any
context of the set and cancels the **whole set**, because the set is recorded and pushed as
one unit. The plan lists every context it cancels. It pushes each receiver without the
`expecting-transfer` entries (Cloudflare's documented cancel) but keeps the forwarded members,
so a caller already rebound to the receiver still reaches `c` on `Sf`. Then it removes the
record. The forwarding goes with the next ordinary deploy of a manifest that no longer puts
`c` on the receiver; a manifest that still does needs `--move-agent` again. From the first landed phase 2 push on, a cancel is refused
and the only way is forward. *Rejected:* requiring `--move-agent` again on resume (a CI re-run would
refuse a half-done move it should finish); cancelling implicitly when the manifest changes
(a typo in `bynk.toml` would undo a move).

### D9. Which moves are supported in the first version

**[DECISION]** Recommend: into a group, out of a group (dissolved, or one that stays: the
`transferred` tombstone is how a class leaves a Worker that keeps running, so
`old_script_stays` becomes movable) and between groups, including **fan-in** (several Workers
with agents forming or joining one group, the case that motivates the feature) and
**fan-out** (a group split across several Workers), each as one move set (§3.1). Still
refused, with new `reason`s on `bynk.deploy.agent_move_refused`:
- a group renamed in place (`renamed_in_place`, as today: its old script would be left with no
  ledger record once the move closes);
- a Worker that is both a source and a receiver in one run (`chained`, e.g. two groups
  swapping members: give one of them a new key, or do it in two manifests and two deploys);
- a `--worker` or `--context` run that leaves out a Worker of the set (`partial_selection`:
  run the whole project);
- a set whose two nodes cannot be ordered (`order`).

If live check S13 shows that one phase 1 push cannot carry entries from several sources (or
S14 that one phase 2 push cannot carry several `transferred_to`s), the set is pushed one
source–receiver pair at a time inside the same phase, each pair's push recorded in `done`,
and the window grows by one push per extra pair. The unit, the record and the user's single
manifest edit stay the same.
*Rejected:* refusing fan-in (it would keep the main adopters of groups off them); asking the
user to edit the manifest one member at a time (each edit is a deploy with its own window,
and the intermediate groups are not ones the user wants).

### D10. Revert refuses across a move; undo is a forward move

**[DECISION]** Recommend §6, with two new codes. *Rejected:* a revert that reverses a move by
transferring back (a revert is a version rollback, and Cloudflare cannot roll back across
the change).

### D11. The window: accepted, measured, documented

Settled by the ruling; recommended form in §5: a plan warning, a guide section with the
measured numbers, and the caller table. No draining.

### D12. Routes and custom domains that change script with the context — **Maintainer ruling needed**

ADR 0874 lists this as an open limit: a domain attaches to one Worker at a time, so a context
whose own routes move to `St` has them on `Sf` until `St` claims them. With phase 1
forwarding `c` (§3.2), `St` can claim them in phase 1 and forward their requests to `Sf`, and
then they are in the window only from phase 2 to phase 3, like every other caller.
**[DECISION]** Recommend that the first version refuses a move where a route or custom domain
would change script (`reason = "routes"`), unless live check S9 shows phase 1 can claim them
cleanly. If it can, they move in phase 1, behind the forwarding. *Rejected:* moving them in
phase 3 (between phase 2 and the claim, requests to the route would reach `Sf` without `c`,
which is still within the window but leaves the claim itself, an account-side change that
can fail, inside it). It is a product call because many real projects reach a context
through a route, and the refusal would keep them off groups.

### D13. `bynk dev` needs nothing

**[DECISION]** Recommend no code. `bynk dev` runs each Worker in its own build directory
(`bynk/src/dev/run.rs`, `current_dir(workers_dir.join(&s.worker))`), so wrangler's local state
(`.wrangler/state` by default, or `--persist-to`,
[wrangler dev](https://developers.cloudflare.com/workers/wrangler/commands/#dev)) belongs to
that Worker, and a context that joins a group starts with empty local agent state. Local
state is disposable; the run-locally guide gets one sentence saying so. *Rejected:* copying
local state between Workers (no user has asked, and it is a second implementation of the
move for a store that is wiped freely).

### D14. A dissolved source becomes a stub, then an orphan

**[DECISION]** Recommend that phase 2 pushes a stub for a source the build no longer has (it
answers `503` and exports `K` in code with the `transferred` tombstones), and that the orphan
report then lists it as "moved its agents to `St`; holds no agent state" instead of the
`fresh_agents` "do not delete" note, once S10 shows deleting it leaves the data on `St`.
Until then the report says it must be kept.

## 9. Slices

Each is one reviewable PR with its own `design/pending/<slug>.md`. The refusal stays exactly
as it is until slice 4 (ruling).

| # | Slice | Done when |
|---|---|---|
| 0 | **Live check** (§10), run by the coordinator on the scratch account | S1–S10 have results posted on #3565, and any result that contradicts §3 or D9–D14 is folded back into this doc |
| 1 | **Ledger and facts shapes.** `agent_moves` record type and its read/write, the `version` 1↔2 rule, `Facts::transfer`, `transferred_classes`; no behaviour uses them | round-trip tests; a ledger with an open record writes `version = 2` and an older-format reader refuses it; a ledger with none is byte-identical to today's; a facts record with `transfer` decodes in the old struct with `transfer` ignored and `classes` without `K` |
| 2 | **Emitter phase variants.** The receive build (forwarded members, `expecting-transfer` entries, no bindings for `K`), the commit config and the dissolved-source stub | golden fixtures for into, out of (staying and dissolved), between groups, fan-in and fan-out; a forwarded member passes a request through unchanged (body and signature); the normal `wrangler.toml` is byte-identical (`positive_fixtures`); the receive config has no binding for `K` |
| 3 | **Planning.** `--move-agent`, move sets, the two-node placement in the order, the new `reason`s, `move_agent_unused`, the plan's move section, the JSON `moves` array and `agent_move_window` warning, `override` change. A non-dry run with a move is still refused (`agent_move_refused`, unchanged) | `--dry-run` and `--format json` show each supported shape and each refusal; `deploy_plan_json.rs` covers them; the refusal is unchanged without `--dry-run` |
| 4 | **Apply and resume.** The three phases, the record before and after each, the account reads, `move_state_unknown`, `move_interleaved`, `move_open`, `--cancel-move`; the refusal is lifted for supported shapes | `apply_tests.rs` fixtures for every row of §3.4, with a fake account and a ledger watch (`ledger::write_watch`) pinning each write; an interrupted push at each phase resumes to completion; phase 3 is never pushed unless the fake account shows the namespace on `St` |
| 5 | **Revert.** `bynk.revert.move_open`, `bynk.revert.move_version`, and the JSON verdict vocabulary | `revert.rs` tests for each row of §6 |
| 6 | **Docs, explain and ADR.** The §5 pages with the measured window, `bynk explain` entries and fixtures for every new error code, the new ADR amending ADR 0874 G5 | docs-consistency and explain-coverage pass; the guide states the measured window from slice 0 |

Slices 1–3 can merge in any order after 0; 4 needs 1–3; 5 needs 1; 6 closes it.

## 10. Live check

Scratch account only, probe Workers only (prefix `moveprobe-`), every script and namespace
deleted afterwards, as V0. Wrangler at `WRANGLER_MIN`. The probe class is V0's `Counter`
(`/set?v=X` stores, `/get` reads) declared through `exports`, `storage = "sqlite"`. "Load" is
a client sending `GET /get` every 100 ms to each named Worker and logging time, status and
body; times are taken from the client's clock and each push's start and end.

| # | Scenario | Steps | Record |
|---|---|---|---|
| S1 | **The window's length** | A holds `Counter` with `hello1`. Load on A and B. Phase 1: B `expecting-transfer`, `transfer_from = A`, no binding. Phase 2: A `transferred`, `transferred_to = B`, class kept in code, binding removed. Phase 3: B plain live with binding. Repeat 5 times | per run: phase 2 start/end, phase 3 start/end, first and last failing request through B and through A, their statuses; p50 and max of "last failure − first failure"; `/get` through B returns `hello1` |
| S2 | **Resume points** | As S1, killing the client's `wrangler deploy` (SIGKILL) during each phase's upload, then repeating that phase | for each phase: whether the kill left the change applied, and whether repeating the phase succeeds and the data survives |
| S3 | **Repeating a phase** | Push phase 1 twice before phase 2; push phase 2 twice before phase 3 | accepted or the error code; data intact |
| S4 | **Source drops or keeps the class code** | S1 with A's class removed from code in phase 2 (V0's form), and with it kept (the docs' form) | both commit or not |
| S5 | **What the namespace listing shows** | `GET /accounts/{id}/workers/durable_objects/namespaces` after each phase | the `script` and `class` of `Counter`'s namespace, and whether a pending transfer appears |
| S6 | **Phase 3 without a commit** | Phase 1 on B, then B plain live with the binding, skipping phase 2 | the error code, or whether B gets an empty namespace (the case the design must never reach) |
| S7 | **Cancel and transfer back** | Phase 1 on B, then B without the `expecting-transfer` entry (cancel); later a full move A→B, then B→A | the cancel's result; whether the move back commits with data intact; any `phase_one_transfer_target_class_provisioned` |
| S8 | **Rollback across a move** | After S1, `wrangler rollback` of B to its phase 1 version, and of A to its pre-move version | the error, to quote in the revert docs |
| S9 | **A route changes script** | A on a route on the scratch zone; move to B with the route in B's phase 1 config (forwarding to A) and removed from A's phase 2 config | whether phase 1 claims the route from A; requests to it during each phase |
| S10 | **Deleting the emptied source** | After S1, `wrangler delete` A | accepted; B still returns `hello1` |
| S11 | **Named environment** | S1 with `--env staging` and env-qualified script names in `transfer_from` / `transferred_to` | commits |
| S12 | **A Service Binding caller** | C bound to B calls `/get` through the binding during S1 | the status and body C receives during the window |
| S13 | **Fan-in** | A and B each hold a class (`Counter`, `Tally`) with data; one phase 1 push of G with both `expecting-transfer` entries, `transfer_from` A and B; phase 2 of A, then of B; phase 3 of G | whether one phase 1 push may name two sources; both data sets survive; the window from A's commit to G's go-live |
| S14 | **Fan-out** | G holds two classes; phase 1 of A and of B; one phase 2 push of G with two `transferred_to`s; phase 3 of A and B | whether one phase 2 push may name two receivers |
| S15 | **Forwarding** | B in phase 1 forwards A's paths to A over a Service Binding; C calls B with a request signed for A | C gets A's answer before phase 2, and the window's answer after it; A verifies the forwarded signature |

S1, S2 and S5 are the ones the ruling asks for; S3 gates the repeat rows of §3.4; S6, S7, S9,
S10, S13 and S14 settle D8, D9, D12 and D14; S15 the phase 1 shape (D2).

## 11. Docs delta

Listed in §5. The book's guide and reference pages change in slices 3–6; its settling PR
adds only this doc and its row in `design/tracks/README.md`.
