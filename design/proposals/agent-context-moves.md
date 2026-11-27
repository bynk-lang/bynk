# Supported moves of agent-owning contexts between Workers

- **Status:** Draft for review. Refs bynk-lang/bynk#3565. It follows on from #3562 (Worker
  grouping, #551) and its V0 verification
  ([#3562 comment](https://github.com/bynk-lang/bynk/issues/3562#issuecomment-6896203799)).
- **Ruling already given (maintainer, 27 November 2026, on #3565):** this goes into 1.0. The
  brief window of `1101` errors between phases 2 and 3 is **accepted and documented** in the
  first version; draining or queueing calls can come later. `bynk.deploy.agent_move_refused`
  stays exactly as it is until this lands.
- **Posture:** a multi-slice increment of the deploy tooling, written in the shape of a track
  doc ([ADR 0076](../decisions/0076-feature-track-posture.md),
  [`tracks/durable-event-delivery.md`](../tracks/durable-event-delivery.md)) because it has
  several `[DECISION]`s and slices. It changes no language surface: no grammar, no checker,
  no editor behaviour (hover, completion, semantic tokens and signature help are unchanged,
  because no source construct is added). It amends
  [ADR 0874](../decisions/0874-worker-groups.md)'s G5 and its "Future work" paragraph.
- **Not a build authorisation.** Each slice in §9 is cut as an ordinary increment proposal.

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
`bynk_project::environment::deployed_worker_name`). A **pair** is all the moves of a run with
the same `(Sf, St)`; a pair is moved as one unit, because one source push and one receiver
push carry all its classes.

### 3.2 The phases

Each phase is one ordinary Worker push through the existing `provisioning.rs:push`
(`wrangler deploy`), of a **phase variant** of the Worker's generated `wrangler.toml` that the
emitter writes beside the normal one (§9, slice 2). The normal provision → secrets → push
steps of `apply.rs:apply_worker` run as for any push; only the config differs.

| Phase | Worker | Config it pushes | Effect |
|---|---|---|---|
| **1, receive** | `Wt` (`St`) | `Wt`'s new build, with each class of `K` as `state = "expecting-transfer"`, `storage = "sqlite"`, `transfer_from = "<Sf>"`, and **no** Durable Object binding for it. A brand-new `St` is a first deploy. | `St` exists and expects the classes. Nothing has moved; the data is still `Sf`'s. |
| **2, commit** | `Wf` (`Sf`) | `Wf`'s new build (or, when `Wf` is no longer a Worker of the build, a **stub**: a `fetch` that answers `503`), with each class of `K` still exported in code, its binding removed, and `state = "transferred"`, `transferred_to = "<St>"`. | Cloudflare reassigns the namespaces to `St`. From here the data is `St`'s. **The window opens.** |
| **3, go live** | `Wt` (`St`) | `Wt`'s normal build: each class plain live, with its binding. | `St` serves `K` with the old data. **The window closes.** |

Placement in the run's order (`gates.rs:ordered`, callee first): phase 1 runs at `Wt`'s slot.
Phases 2 and 3 run **back to back**, with no prompt and nothing else between them, at the
later of `Wt`'s and `Wf`'s slots, so `Sf`'s own callees are live before it is pushed. `Wf`'s
ordinary push is replaced by phase 2 (it is pushed once). The rest of the order then goes on
as usual, which is where the callers that bound `Sf` for `c` are pushed with their bindings
pointing at `St`.

### 3.3 What is recorded, before and after each phase

The ledger gains one record per open pair (§4). Each line below is one `write_lock`, through
the same `LedgerWriter` the run already uses, so each is atomic and durable.

| When | Ledger: the pair's `step` | Also recorded |
|---|---|---|
| Before phase 1 | `receiving` | `from`, `to`, `contexts` (context → classes), `started`, the bynk version; the ledger `version` becomes 2 (D5) |
| After phase 1 succeeds | `received` | `St`'s new live deployment id (read from the deployments listing) |
| Before phase 2 | `committing` | `Sf`'s live deployment id as read just before the push |
| After phase 2 succeeds | `committed` | `Sf`'s new deployment id; `Sf`'s Worker record as for any push (its members no longer list `c`; the `transferred` tombstones in a new `transferred_classes` map beside `deleted_classes`) |
| Before phase 3 | `going_live` | nothing more |
| After phase 3 succeeds | record **removed** | `St`'s Worker record as for any push (`contexts`, `script`, contracts); the ledger `version` returns to 1 when no record is open |

A phase that fails leaves the record at its "before" step and the run stops, as any failed
push does today (`apply.rs:apply_plan`, `stopped_report`). An interrupted push
(`Pushed::Interrupted`) is treated the same way: not recorded as done.

### 3.4 What a re-run does from each recorded step

A re-run reads the open record and **resumes the same pair** without `--move-agent` being
passed again (the record is the consent already given), provided the manifest still asks for
the same move. Before each push it reads the account (D7): the namespace listing for each
class of `K` (`sweep.rs:Cloudflare::namespaces`, which reports a namespace's `script`) and the
deployments of `Sf` and `St`.

| Recorded step | What the account shows | The re-run |
|---|---|---|
| `receiving` | `St` has no deployment newer than the record, or its newest version's facts do not mark the transfer | repeats phase 1 |
| `receiving` | `St`'s newest version's facts mark `expecting` for `K` | records `received`, goes to phase 2 |
| `received` | `K`'s namespaces still belong to `Sf` | phase 2 |
| `committing` | `K`'s namespaces still belong to `Sf` | repeats phase 2 |
| `committing` | `K`'s namespaces belong to `St` (the push landed but was not recorded) | records `committed`, goes to phase 3 |
| `committed`, `going_live` | `K`'s namespaces belong to `St` | phase 3 (repeating it is a plain redeploy) |
| `committed`, `going_live` | `K`'s namespaces belong to `Sf`, or to neither | **stops**: `bynk.deploy.move_state_unknown`, naming the classes and both scripts. Never declares `K` live on `St` (§7.1) |
| any | a deployment of `Sf` or `St` newer than the one recorded, made by anything else | **stops**: `bynk.deploy.move_interleaved` (§7.2) |
| any | the manifest no longer asks for this move | **stops**: `bynk.deploy.move_open` (D8) |

**Resuming is idempotent.** The rule behind the rows above: before each phase, the re-read
asks whether that phase's effect is already on the account, and if it is, the phase counts as
already applied. The run records the "after" step without pushing and goes on to the next
phase. In particular, when the re-read before phase 2 finds `K`'s namespaces already owned by
`St`, phase 2 is not pushed again: the run records `committed` (with `Sf`'s newest deployment
id) and goes straight to phase 3. When the re-read before phase 1 finds `St`'s newest version
already marking `expecting` for `K`, phase 1 is not repeated. So however many times a run is
interrupted and started again, each phase is applied at most once, and only phase 3, a plain
redeploy, is ever pushed twice.

The namespace listing is the single source of truth for "who holds the data", and the ledger
only says where the run had got to. This keeps ADR 0194 D1's rule that the ledger records
what was pushed and never claims to know the namespaces (`ledger.rs:Environment::deleted_classes`'s
comment; the "Understand" box in the deploy guide's *Durable Objects* section). Whether the
listing shows the reassignment at phase 2, and whether it shows an `expecting-transfer`
namespace at all, is live check S5; if it does not, the resume reads the facts of the newest
versions of `Sf` and `St` instead, and the `committing` row becomes "repeat phase 2", which
must then be shown to be safe (S3).

## 4. The ledger and `BYNK_DEPLOY_FACTS`

### 4.1 The ledger

A new, additive table under each environment (`ledger.rs:Environment`), `default`ed and
omitted when empty, like every field added since v1:

```toml
[[environments.default.agent_moves]]
from_worker = "shop-orders"
from_script = "shop-orders"
to_worker = "core"
to_script = "core"
step = "committed"            # receiving | received | committing | committed | going_live
started = "2026-11-27T10:15:02Z"
bynk = "…"                   # the bynk version that started it
from_deployment = "…"         # the deployment id each side was last seen at
to_deployment = "…"

[environments.default.agent_moves.contexts]
"shop.orders" = ["Cart"]
```

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
{"v":1, "classes":["Other"], "transfer":{"expecting":{"Cart":"shop-orders"}}, "contexts":["shop.orders","shop.pay"]}
```

- A phase 1 version of `St` carries `transfer.expecting` (class → source script) and does
  **not** list `K` in `classes`, because it declares them pending, not live.
- A phase 2 version of `Sf` carries `transfer.transferred` (class → receiver script) and does
  not list `K` in `classes`.
- A phase 3 version carries no `transfer` and lists `K` in `classes`.

Keeping `K` out of `classes` while it is not live means a bynk that does not know the
`transfer` field (it decodes it as absent, since `Facts` does not deny unknown fields) still
refuses to revert across a move by the existing `bynk.revert.class_changed`. Bumping
`FACTS_VERSION` was rejected: `Facts::decode` reads any other version as "no facts", which
would make every version written by the new bynk unrevertible to an older one. The field is
a few dozen bytes, well inside `MAX_FACTS_BYTES`.

### 4.3 Stored-state encoding

The move does not touch the stored-state encoding that `design/bynk-1.0-definition.md` freezes
at 1.0 (ADR 0538): Cloudflare reassigns the namespace and no record is read or rewritten by
Bynk. The class's schema version (`Facts::agents`) and storage layout (`Facts::layout`) go
with the class to `St`, the schema lock is keyed by `<context>.<Agent>` and does not change,
and `bynk migrate`'s per-Worker sweep follows the class through `migrate::worker_of`. The
ledger and the facts are tooling surface, versioned with the toolchain, and both changes are
additive except the temporary `version = 2`, which is there to stop an older reader.

## 5. The error window

**Accepted** (ruling). Between phase 2 committing and phase 3 landing, a request that reaches
`K` on `St` fails (`1101`, observed in V0), and a caller still bound to `Sf` reaches either
`Sf`'s new build without `c` (its router's 404) or the stub (503). The window is two pushes
long at best: the end of phase 2's push plus the whole of phase 3's push, and Cloudflare's
docs add that phase 3 should follow "once the source rollout is complete", which is also
propagation time. The live check (S1) measures it; until then the docs give no number.

**What a caller sees:**

| Caller | During the window | After |
|---|---|---|
| A route or custom domain on `St` that reaches `K` | `500` (Cloudflare error `1101`) | normal |
| Another member of `St`'s group calling `c` in-process | the callee's throw, answered as the plain `500` a Service Binding gives, which a Bynk caller receives as a callee fault (ADR 0874, G3-2) | normal |
| A Worker still bound to `Sf` for `c` | `404` from `Sf`'s router, or `503` from the stub, until that caller's own push rebinds it to `St` later in the same run | normal |
| Anything that does not touch `c` | unaffected | — |

A caller pushed later in the order is in the window until its own push: the plan says which
ones they are.

**Where the docs say it:**

- the plan: `bynk deploy` (and `--dry-run`) prints a move section naming each pair, its
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
   from `committed`/`going_live`, after the account shows `K`'s namespaces on `St` (§3.4).
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
- each phase re-reads `Sf`'s and `St`'s deployments and compares them with the ids recorded at
  the previous step, so a deploy from elsewhere in between stops the run
  (`bynk.deploy.move_interleaved`), in the style of revert's `Stopped::Moved`;
- a second checkout without the open record (the ledger not yet committed) plans the move
  afresh; its pre-phase-1 read finds `K` already on `St` or pending, and it stops with
  "the move is already under way or done: pull `bynk.deploy.lock`".

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

### D2. Phases 2 and 3 back to back, at the later of the two Workers' slots

**[DECISION]** Recommend phase 1 at `Wt`'s slot, then phases 2 and 3 back to back at the
later of `Wt`'s and `Wf`'s slots, with callers rebound by the rest of the order (§3.2).
*Rejected:* pushing every caller of `c` before phase 2 (their agent paths would fail from
their own push to phase 3, making the window longer and spread over more Workers); one run
per phase (three runs, two of them with an outage open between them); a separate
`bynk move` command (the move is a consequence of the manifest the deploy already plans).

### D3. The phases are generated config variants pushed by the existing push

**[DECISION]** Recommend the emitter writes `wrangler.receive.toml` and `wrangler.commit.toml`
(names to settle in slice 2) beside the Worker's normal config, and a stub Worker for a
dissolved source, and the push takes the config path. *Rejected:* editing the generated
`wrangler.toml` in place at deploy time (the build tree stops describing what was built); a
REST upload (ADR 1075 keeps the push on wrangler, and `wrangler versions upload` cannot carry
lifecycle changes).

### D4. The ledger records the pair and its step, before and after each phase

**[DECISION]** Recommend the additive `agent_moves` table and the five steps of §3.3.
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
phase 2 the user may cancel with `--cancel-move <context>`, which pushes `St` without the
`expecting-transfer` entries (Cloudflare's documented cancel); from `committed` on, the only
way is forward. *Rejected:* requiring `--move-agent` again on resume (a CI re-run would
refuse a half-done move it should finish); cancelling implicitly when the manifest changes
(a typo in `bynk.toml` would undo a move).

### D9. Which moves are supported in the first version

**[DECISION]** Recommend: into a group, out of a group (dissolved, or one that stays: the
`transferred` tombstone is how a class leaves a Worker that keeps running, so
`old_script_stays` becomes movable) and between groups. Still refused, with new `reason`s on
`bynk.deploy.agent_move_refused`: a group renamed in place (`renamed_in_place`, as today: its
old script would be left with no ledger record once the move closes); a Worker in more than
one pair in one run (`chained`: run them one after another); a `--worker` or `--context` run
that leaves out either Worker of a pair (`partial_selection`). *Rejected:* supporting chains
in v1 (the phase ordering across several pairs is untested and V0 did not probe a second
transfer).

### D10. Revert refuses across a move; undo is a forward move

**[DECISION]** Recommend §6, with two new codes. *Rejected:* a revert that reverses a move by
transferring back (a revert is a version rollback, and Cloudflare cannot roll back across
the change).

### D11. The window: accepted, measured, documented

Settled by the ruling; recommended form in §5: a plan warning, a guide section with the
measured numbers, and the caller table. No draining.

### D12. Routes and custom domains that change script with the context — **Maintainer ruling needed**

ADR 0874 lists this as an open limit: a domain attaches to one Worker at a time, so a context
whose own routes move to `St` has them on `Sf` until `St` claims them. **[DECISION]**
Recommend that the first version refuses a move where a route or custom domain would change
script (`reason = "routes"`), unless live check S9 shows phase 3 can claim them cleanly; if
it can, they move in phase 3, and the window covers them. *Rejected:* moving them in phase 1
(traffic would reach `St` before it has the data). It is a product call because many real
projects reach a context through a route, and the refusal would keep them off groups.

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
| 2 | **Emitter phase variants.** The receive and commit configs and the dissolved-source stub | golden fixtures for into, out of (staying and dissolved) and between groups; the normal `wrangler.toml` is byte-identical (`positive_fixtures`); the receive config has no binding for `K` |
| 3 | **Planning.** `--move-agent`, pairs, placement in the order, the new `reason`s, `move_agent_unused`, the plan's move section, the JSON `moves` array and `agent_move_window` warning, `override` change. A non-dry run with a move is still refused (`agent_move_refused`, unchanged) | `--dry-run` and `--format json` show each supported shape and each refusal; `deploy_plan_json.rs` covers them; the refusal is unchanged without `--dry-run` |
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
| S9 | **A route changes script** | A on a route on the scratch zone; move to B with the route in B's phase 3 config and removed from A's phase 2 config | whether phase 3 claims the route; requests to it during phases 2–3 |
| S10 | **Deleting the emptied source** | After S1, `wrangler delete` A | accepted; B still returns `hello1` |
| S11 | **Named environment** | S1 with `--env staging` and env-qualified script names in `transfer_from` / `transferred_to` | commits |
| S12 | **A Service Binding caller** | C bound to B calls `/get` through the binding during S1 | the status and body C receives during the window |

S1, S2 and S5 are the ones the ruling asks for; S6, S7, S9 and S10 settle D8, D9, D12 and
D14.

## 11. Docs delta

Listed in §5. The book's guide and reference pages change in slices 3–6; this doc is the
only change in its own PR.
