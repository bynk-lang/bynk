# Agent alarms — an agent schedules its own future work

- **Status:** Settling in the draft PR that adds this doc. Spine issue: #3934,
  which carries the live slice status.
- **Realises:** `design/bynk-design-notes.md` §7 ("Alarm": *"the agent uses an
  `Alarms` capability to schedule a future wake-up and exposes a handler the
  runtime invokes when the time arrives"*, line 154) and §6 (*"alarm firings
  are at-least-once"*, line 133). Also the active eviction that
  [ADR 0113](../decisions/0113-cache-ttl-eviction.md) deferred ("active/
  alarm-based reaping", line 96) and
  [ADR 0791](../decisions/0791-cache-reaping-at-commit.md) left for later.
- **Posture:** Feature track per
  [ADR 0076](../decisions/0076-feature-track-posture.md), which named Alarm as
  an obvious future track (line 81). It qualifies on two axes (§1.2). It is not
  a security boundary (§13).
- **Governing records:**
  [ADR 0109](../decisions/0109-handler-atomic-commit.md) (the handler is the
  atomic unit for storage),
  [ADR 0781](../decisions/0781-serialise-agent-handlers.md) (one handler at a
  time per instance; alarms are already an entry point),
  [ADR 0523](../decisions/0523-the-test-harness-owns-the-clock.md) (the harness
  owns the `Clock`; this track amends its part C),
  [ADR 0113](../decisions/0113-cache-ttl-eviction.md) and
  [ADR 0791](../decisions/0791-cache-reaping-at-commit.md) (`Cache` expiry and
  the clock it is judged by),
  [ADR 0438](../decisions/0438-agents-sqlite-durable-objects.md) (agents are
  SQLite-backed Durable Objects and use no alarms). Four ADRs land up front
  (§15).

## 1. The theme

### 1.1 What this track settles

An agent acts only when something calls it. It can't expire an unpaid order,
send a reminder, retry later, time out a session or evict its own `Cache`
entries on a schedule. Today's workaround is a cron service. A cron handler has
to know which instances to wake, and agents can't be enumerated, so the
workaround keeps a second index of "instances with pending work" by hand.

This track gives an agent typed timers. When it retires:

1. an agent declares **named, typed timers**, each with its own handler;
2. a handler **schedules, replaces and cancels** them, for its own instance
   only, as part of its atomic commit;
3. the runtime fires them **at least once** from the Durable Object alarm, many
   logical timers multiplexed onto the platform's one alarm per object;
4. tests fire them on the **harness clock** with `advance Clock by`, and
   directly with a `fire` statement;
5. `Cache` entries are **reaped actively** on the same mechanism.

### 1.2 Why a track (the ADR 0076 trigger)

- **It spans many increments.** The surface and handler, the stored schedule
  and platform alarm, test-tier firing, the bundle target, `self.erase()`
  (#3931) and migrations, and active `Cache` eviction are each reviewable on
  their own (§14).
- **Its surface is not settled.** One handler or named timers, payloads, timer
  identity, and whether scheduling is a capability or a member of `self` are
  all open (§16).
- **Not a security boundary.** Timers are self-addressed and never reachable
  from a request (§13). Atomicity and at-least-once delivery are correctness
  questions, settled in §6 and §7.

## 2. What's true today

### 2.1 The runtime

- **No alarm is ever set.** The emitted agent persists through `get`, `put`,
  `list` and `transaction`; ADR 0438 records that it uses no alarms
  (`0438-agents-sqlite-durable-objects.md:16-19`). The runtime's storage
  interfaces have no alarm methods: `DurableObjectTransaction`
  (`bynk-emit/runtime/src/storage.ts:20-29`) and `DurableObjectStorage`
  (`storage.ts:31-42`) stop at `get`/`put`/`delete`/`list`/`transaction`, and
  the in-memory storage the tests use (`storage.ts:105`) has none either.
- **The queue already expects alarms.** `serialiseAgentClass` wraps every entry
  point of an agent class in the instance's one-at-a-time queue, and its list
  already names `alarm` (`bynk-emit/runtime/src/agent.ts:665`). An emitted class
  has no `alarm` method, so the wrapper skips it (`agent.ts:693`). ADR 0781 D1
  lists "an alarm" among the entry points the queue serialises
  (`0781-serialise-agent-handlers.md:9`).
- **The wait limit.** A call waits at most `AGENT_QUEUE_WAIT_MS = 30_000`
  behind the handlers ahead of it (`agent.ts:457`), then faults `AgentBusy`.
  #3932 proposes `@busyAfter(d)` to lower it.
- **The atomic commit.** Per-entry storage stages every put and delete in an
  `EntryOverlay` (`bynk-emit/runtime/src/entries.ts:130`) and writes them, with
  the `"state"` record, in one `transaction` (`entries.ts:205-228`). A faulting
  handler discards the overlay. Expiry keys already have a shape of their own
  (`entries.ts:104-109`), which ADR 0791 uses to reap `Cache` entries at commit,
  at most 64 per commit.
- **A Durable Object agent provisions itself.** On `workers` the object
  rebuilds `env`, its `given` providers and its events fan-out from its own
  `this.__env` (`agent.ts:139-147`), so a handler invoked with no caller still
  has its capabilities. On `bundle` an agent call threads the caller's `deps`
  (`bynk-emit/src/emitter/lower_call/agents.rs:27-29`), and the bundle has no
  place of its own to compose an agent's capabilities: that is the open
  [DECISION B1] of [`agent-capability-encapsulation.md`](agent-capability-encapsulation.md) §3.
- **Bundle agents live in memory.** On `bundle` an instance's state is an
  in-memory `DurableObjectState` held by a `StateRegistry` (`agent.ts:52-68`);
  the bundle serialises calls with a proxy over that state (`agent.ts:557`).

### 2.2 The language

- An agent's only handler kind is `on call`. `HandlerKind`
  (`bynk-syntax/src/ast.rs:1969-1996`) has `Call`, plus service kinds
  (`Http`, `Cron`, `Message`, `Open`, `Close`, `Event`, …). `event` is matched
  as plain identifier text after `on`, with no reservation (`ast.rs:1990-1995`);
  a `timer` kind can follow the same rule.
- A handler's `store` writes commit "atomically when the handler returns",
  and a handler that faults persists nothing
  (`site/src/content/docs/book/reference/agents.md:129-131`).
- `self.<key>` reads the instance's key (`agents.md:27`); `self` is a hard
  keyword for "the current agent instance" (`reference/keywords.md:75`).
- `Cache` expiry reads `given Clock`, which the checker enforces
  (`bynk.store.cache_needs_clock`; ADR 0113 D4,
  `0113-cache-ttl-eviction.md:54-62`).
- Cron handlers can't be declared in an agent (`reference/cron.md`, "Placement").

### 2.3 Tests and time

- Each case has one harness clock, starting at the epoch; `advance Clock by`
  moves it (`reference/testing.md:1467-1495`).
- The harness clock already has a **timer queue** at `unit` and `integration`:
  `within` and `after` stubs register on it, and `advance` fires every timer
  due by then, earliest first (`testing.md:1141-1144`;
  `spec/static-semantics.md:2624-2627`). The implementation fires them in one
  synchronous loop (`bynk-driver/src/project/test_runtime/virtual_time.ts:40-48`);
  `__bynkAdvanceClock` returns `void`, and the emitter lowers `advance` to a
  plain call (`bynk-emit/src/emitter/lower_stmt.rs:1008-1021`).
- **Durable Object alarms are excluded from the harness clock today.** ADR 0523
  C: "The runtime's own clocks (JWT `exp`, the `Signature` tolerance, Durable
  Object alarms) keep wall-clock time" (`0523-the-test-harness-owns-the-clock.md:11`;
  `testing.md:1499-1502`). No alarm existed to test, so this cost nothing.
- At `system`, participants run in process with in-memory Durable Object
  namespaces (ADR 0467; `agent.ts:276-301`). Cron and queue addresses drive the
  Worker's real `scheduled` and `queue` entry points (`testing.md:485-520`).

### 2.4 What the platform gives

What this doc relies on, and what still needs checking against Cloudflare's
documentation before slice 1:

- **One alarm per object.** `setAlarm(t)` replaces any alarm already set;
  `getAlarm()` reads it; `deleteAlarm()` clears it. The object's `alarm()`
  handler runs at or after `t`. Confident.
- **At-least-once.** An alarm can run more than once. If `alarm()` throws, the
  platform retries it with exponential backoff. *To confirm:* the starting
  delay and retry count (commonly quoted as 2 seconds and six retries), and the
  `alarmInfo` argument (`retryCount`, `isRetry`).
- **An alarm wakes an evicted or hibernating object.** The constructor runs
  again; nothing in memory survives. Confident.
- *To confirm:* whether `setAlarm` inside `storage.transaction()` commits with
  the transaction; whether `deleteAll()` also clears the alarm (it has not
  always); the wall-time limit on one `alarm()` invocation; any upper bound on
  how far ahead an alarm may be set. The design below does not depend on the
  answers (§5.3), but the ADRs should state them.

## 3. Goals and non-goals

**Goals**

- Named, typed timers declared by their handlers, with a payload.
- Scheduling, replacing and cancelling from any of the agent's own handlers,
  committed atomically with its other writes.
- Firing at least once, in due order, through the instance's queue, with a
  runtime-owned retry and a visible end when retries run out.
- Deterministic tests at every in-process tier, using the existing harness
  clock.
- Active `Cache` eviction that needs no handler to run.
- A timer handler looks like any other handler: the same `given`, `store`
  access, events and faults (design notes §7, "handler uniformity").

**Non-goals**

- **Scheduling another instance's timers.** An agent can only schedule for
  itself. To make another instance act later, call it and let it schedule.
- **Recurring timers** (`every(1.hours)`). A handler re-schedules itself; a
  recurring form can be added later if the pattern proves common.
- **Cross-agent workflows and compensation.** That is #3185 (sagas). This track
  gives sagas a possible substrate, not a design.
- **Listing pending timers from outside**, or a cron-like global schedule.
- **Exactly-once execution.** The contract is at-least-once (§6).
- **Durable timers on `bundle`.** Bundle state is in memory, and so are its
  timers (§9).

## 4. The surface

### 4.1 Declaring a timer

A timer is declared by its handler. `on timer <name>(<params>)` declares the
timer and the code that runs when it fires. The parameters are its payload.

```bynk
context shop

consumes bynk { Clock, Events }

type OrderId = String where Matches("[a-z0-9-]{1,40}")

event OrderExpired = {
	order: OrderId,
}

agent Basket {
	key id: String

	store unpaid: Map[OrderId, Int]

	on call place(order: OrderId, cents: Int) -> Effect[()] given Clock {
		let _ <- unpaid.put(order, cents)
		do self.timers.expire(order).after(30.minutes)
	}

	on call pay(order: OrderId) -> Effect[()] {
		let _ <- unpaid.remove(order)
		do self.timers.expire(order).cancel()
	}

	on timer expire(order: OrderId) -> Effect[()] given Events {
		let open <- unpaid.contains(order)
		if open {
			let _ <- unpaid.remove(order)
			do Events.emit[OrderExpired](OrderExpired { order: order })
		}
	}
}
```

Rules (diagnostic names are proposals):

- **Agent only.** `on timer` is legal only in an `agent`
  (`bynk.timer.outside_agent`).
- **Return type `Effect[()]`.** A timer has no caller to answer
  (`bynk.timer.return_not_unit`). This is the side-effect shape the design notes
  give alarms (§7, line 160).
- **Parameters are `Key` types.** Each parameter must be a
  [`Key`](/book/reference/types/#keys) type (`bynk.timer.param_not_key`). The
  payload is stored and is part of the timer's identity (§4.3), so it needs a
  stable equality and a wire form. A larger payload goes in the agent's `store`,
  keyed by the timer's parameters.
- **Not callable.** A timer handler is not part of the agent's call surface.
  `Basket(id).expire(order)` and `this.expire(order)` are refused
  (`bynk.timer.not_callable`). Shared logic goes in a function.
- **No `by` clause**, as for `on call` (`bynk.actor.by_on_agent`).
- **`given` as any handler.** A timer handler declares its capabilities the
  usual way, and implied requirements (`Cache`, `Log`) apply.

### 4.2 Scheduling

Scheduling is a member of `self`, not a capability. `self.timers.<name>(args)`
names one timer; three operations act on it, each `Effect[()]`:

| Operation | Meaning |
|---|---|
| `.after(d: Duration)` | due at `Clock.now() + d`; implies `given Clock` (`bynk.timer.needs_clock`) |
| `.at(t: Instant)` | due at `t`; needs no clock |
| `.cancel()` | removes the pending timer with this identity; a no-op if there is none |

```bynk
agent Session {
	key id: String

	store lastSeen: Cell[Instant]

	on call touch() -> Effect[()] given Clock {
		let now <- Clock.now()
		lastSeen := now
		do self.timers.timeout().after(20.minutes)
	}

	on timer timeout() -> Effect[()] {
		do self.erase()
	}
}
```

Each `touch` replaces the pending `timeout`, so the session times out twenty
minutes after the last touch. (`self.erase()` is #3931.)

- **Own instance only, by construction.** `self` exists only inside the agent's
  own handlers, and an agent reference (`Basket(k)`) exposes only `on call`
  handlers. There is no expression that names another instance's timers.
- **Where it is legal.** In `on call` and `on timer` handlers. Not in
  `migrate` steps, invariants, transitions or functions
  (`bynk.timer.schedule_outside_handler`).
- **A negative duration** can only be computed, and faults the handler, as a
  negative `advance` amount does.

### 4.3 Identity, replacement and cancellation

- **Identity is the name and the payload.** `expire("o1")` and `expire("o2")`
  are two timers; `expire("o1")` scheduled twice is one.
- **Scheduling an identity that is pending replaces it.** Its due time moves,
  earlier or later. The last schedule in a handler wins.
- **`cancel()` is idempotent.**
- **Within one handler, the operations apply in order** against the staged
  schedule: schedule then cancel leaves nothing; cancel then schedule leaves
  the new one.
- **A zero-parameter timer** (`timeout()`) has exactly one identity, so it is a
  single replaceable timer.

## 5. Scheduling semantics

### 5.1 The stored schedule

The platform gives one alarm per object, so the runtime keeps the schedule
itself, in the instance's storage, in the per-entry layout:

- one entry per pending timer under a due-ordered key (due instant, then a
  sequence number for ties), holding the timer name, encoded payload and attempt
  count;
- one identity entry per pending timer, pointing at its due key, so replace and
  cancel are single lookups.

Both are written through the same `EntryOverlay` as the handler's other writes
(§7). The exact key shapes belong to slice 1, following the vocabulary in
`entries.ts` (padded numbers, encoded key parts).

### 5.2 Which clock

Due instants are **`Clock` instants**, read from the **first-party `Clock`**,
implied for every agent that declares a timer. Neither the scheduling handler nor
the timer handler declares `given Clock` for it: `after(d)` reads the first-party
clock to stamp the entry (`.at(t)` needs no clock at all), and the alarm
dispatch reads the same clock to judge what is due (as ADR 0791 judges expiry by
the clock the program read). A context's own `provides Clock` is not used for
either, so stamping and dispatch can never read different clocks. On Cloudflare the first-party `Clock` is wall-clock time, so the platform
alarm is set to the earliest due instant and the two agree. Under the test
harness the `Clock` is the harness clock, so `advance Clock by` decides what is
due (§8). This amends ADR 0523 C for agent timers: they move with the harness
clock, because they were stamped by it. JWT `exp` and the `Signature` tolerance
keep wall-clock time, as before.

### 5.3 Arming the platform alarm: early, never late

After a handler changes the schedule, the platform alarm must be at or before
the earliest pending entry. The runtime arms it **before** the flush, to the
earlier of the current alarm and the new earliest entry, and corrects it
**after** the flush, to the true earliest (or deletes it when nothing is
pending).

- A spurious alarm is harmless: the dispatch finds nothing due and re-arms.
- A lost alarm is not. Arming first means a crash between the alarm call and the
  flush can only leave an early alarm, never a missing one.
- So the design holds whether or not `setAlarm` joins the storage transaction
  (§2.4, to confirm). If it does, slice 1 can do both in the transaction and
  drop the second step.

## 6. Firing, retries and at-least-once delivery

### 6.1 The dispatch

When the platform alarm fires, the emitted `alarm()`:

1. reads the first-party `Clock` and lists entries due by then, in due order, at
   most a batch per wake (proposed 64, matching ADR 0791's reaping budget);
2. runs **each timer as its own handler**, through the instance's queue: lease
   (§6.3), load, body, commit. The timer's entry is deleted **in that handler's
   commit**, with its other writes. The deletion is staged **before** the body
   runs, against the entry's old due key, so a body that schedules the same
   timer again (`self.timers.tick().after(1.hours)`, the recurring pattern)
   writes a fresh entry that survives the commit instead of being cancelled by
   its own firing;
3. re-arms the alarm to the next pending entry (immediately, if the batch left
   due entries behind).

Each timer takes its own place in the queue, so a backlog of timers does not
hold callers off for the whole batch. Today `serialiseAgentClass` would wrap
`alarm` as one entry (`agent.ts:665`); slice 1 changes the alarm path to enqueue
per timer.

### 6.2 At-least-once, and what it means for the author

A timer's entry is removed only when its handler commits. So:

- a handler that **faults** leaves its timer pending, to be retried (§6.3);
- an object **evicted or crashed** mid-handler commits nothing, and the
  platform alarm (still set, or retried) runs it again;
- a timer handler's **`store` writes and events** happen once per committed
  run: writes commit with the entry's deletion, and events are released only at
  commit (ADR 0781 D5);
- its **external effects** (a `Fetch`, a `Queue` send, an `Email`) can repeat,
  because they happen before the commit. The Book tells authors to make them
  idempotent, with `Idempotency.claim` keyed by the timer's payload.

### 6.3 Retries are the runtime's, not the platform's

A timer handler that faults is **not** allowed to make `alarm()` throw. The
dispatch catches the fault, reports it through the ordinary fault path (agent and
timer name, never the key or payload, as ADR 0107 does for invariants), bumps
the entry's attempt count and moves its due time back by a backoff.

- **Why not the platform's retry.** One platform alarm carries many timers. A
  throw would re-run every timer in the batch, the platform's retry policy is
  not ours to set or to test, and its count may change.
- **Proposed policy.** Doubling backoff from 30 seconds, at most 5 attempts.
  Both are named constants, set in the ADR.
- **After the last attempt** the entry is removed and the runtime logs and
  reports `TimerAbandoned <Agent>.<timer>` with the attempt count. That is the
  dead-letter: visible in logs and fault reports, not stored. A stored
  dead-letter list, or an author-declared `on timer … abandoned` hook, are
  follow-ons if logs prove not enough.
- **Leasing, so a crash counts as an attempt.** A caught fault is not the only
  way a timer fails: a handler that kills `alarm()` outright (a CPU or
  wall-time limit, running out of memory) never reaches the catch. So before
  running a timer's body, the dispatch writes a **lease**: it bumps the
  entry's attempt count and moves its due time out by the next backoff, in a
  write of its own that commits before the body starts. A commit then deletes
  the entry; a caught fault needs no further write; a crash leaves the leased
  entry due again after the backoff, with the attempt already counted. A
  crash-looping timer therefore reaches `TimerAbandoned` like any other, and
  cannot stall the instance's other timers once the platform stops retrying.
- **The platform's retry still covers the runtime itself**: if `alarm()` dies
  outside a timer handler (an eviction, a storage failure), the platform runs it
  again and the stored schedule is unchanged.

## 7. The atomic commit, the queue and hibernation

- **Schedule changes are store writes.** `after`, `at` and `cancel` stage
  entries in the overlay and flush in the handler's one transaction
  (`entries.ts:205-228`). A handler that faults schedules and cancels nothing.
  Invariants see no timer state; a timer is not part of the agent's
  observable state.
- **One handler at a time.** A timer handler is an entry point like any other
  (ADR 0781 D1). It waits in the instance's queue, it runs alone, and a handler
  that reaches its own instance again faults `ReentrantAgentCall`. A timer starts
  a fresh call chain.
- **Queue wait.** A timer waits for the handlers ahead of it with the agent's
  limit (30 seconds, or `@busyAfter` from #3932). If it gives up, it stays
  pending and re-arms shortly. That is **not** counted as an attempt and not
  reported as a fault; it is logged as busy, as #3932 B proposes for HTTP.
- **Hibernation and eviction.** Nothing about timers lives in memory. An alarm
  wakes an evicted object, the constructor runs, and the dispatch reads the
  schedule from storage. Hibernated WebSockets (ADR 0133) are unaffected, and a
  timer handler can send on a held `Connection` like any handler.
- **Capabilities.** An alarm has no caller and so no forwarded `deps`. On
  `workers` the object already rebuilds its `given` providers from its own
  `env` (`agent.ts:139-147`), so a timer handler gets them the same way. Bundle
  is §9.

## 8. Testing

### 8.1 `advance Clock by` fires due timers

At `unit` and `integration`, the in-memory storage gains the alarm methods, and
the harness keeps the set of instances with an armed alarm. `advance Clock by d`
then:

1. takes the earliest due item across the harness's timer queue (`within`,
   `after` stubs) and every armed agent alarm, up to the target time;
2. moves the clock to that item's time and runs it; an agent alarm runs the
   dispatch of §6.1 and is **awaited** to completion;
3. repeats, so a timer that schedules another inside the window fires in the
   same `advance`;
4. sets the clock to the target.

Ties go to schedule order, across everything on the harness clock: one
harness-wide sequence number stamps every timer, stub `after` and agent alarm
when it is scheduled, so two instances' timers, or a timer and a stub, due at
the same instant always run in the same order. `advance` becomes an awaited
statement in a suite from which an agent that declares timers is **reachable**
(through its target's agents and their calls), not merely one whose own module
declares them; every other suite keeps today's synchronous lowering byte for
byte, as virtual time does today (`testing.md:1141-1144`).

```bynk
suite shop.basket {
	case "an unpaid order expires after thirty minutes" {
		do Basket("b1").place("o1", 500)
		advance Clock by 29.minutes
		expect Events.emit never called
		advance Clock by 1.minutes
		expect Events.emit called once
	}

	case "paying cancels the expiry" {
		do Basket("b1").place("o1", 500)
		do Basket("b1").pay("o1")
		advance Clock by 1.hours
		expect Events.emit never called
	}
}
```

A timer handler that faults during `advance` is retried on the harness clock
per §6.3, so a case can advance through the backoff and see `TimerAbandoned`.

### 8.2 `fire` runs a timer now

```bynk
suite shop.basket {
	case "expiry is idempotent" {
		do Basket("b1").place("o1", 500)
		fire Basket("b1").expire("o1")
		fire Basket("b1").expire("o1")
		expect Events.emit called once
	}
}
```

`fire <Agent>(<key>).<timer>(<args>)` is a statement legal only in a `case`
body (`bynk.test.fire_outside_case`), contextual like `advance`. It runs the
timer handler now, through the queue, as the dispatch would: a pending timer
with that identity is removed by its commit, and none needs to be pending. A
fault faults the case, so `expect fire … faults` reads as `expect <call> faults`
does. It is the way to test at-least-once behaviour (fire twice) without time.

### 8.3 `system` and remote runs

- **`system`.** Participants use in-memory Durable Object namespaces (ADR 0467),
  so the same harness mechanism drives each armed instance's real `alarm()`
  entry point. A case that passes at `unit` promotes unchanged, as ADR 0523
  intends. `fire` reaches the object through a reserved internal route that
  exists only in a test build, as the `__bynkClock` seam does
  (`0523-the-test-harness-owns-the-clock.md:14`).
- **`--against` a deployment.** `advance` is already refused
  (`bynk.test.remote_unsupported_advance`). `fire` is refused the same way
  (`bynk.test.remote_unsupported_fire`); a deployed build carries no fire route.

## 9. Targets: `bundle`, Node and the browser

The `bundle` target runs in Node and in the browser, with agent state in memory
(`StateRegistry`, `agent.ts:52-68`). Timers there run **in process**:

- the schedule lives in the instance's in-memory storage, exactly as on
  `workers`, so the same dispatch code runs;
- the "platform alarm" is a host timer through a seam like the one `within`
  uses (`exec.timers`, `bynk-emit/runtime/src/settle.ts:74-79`), re-armed in
  chunks no longer than 2,147,483,647 ms, the longest a host timer takes
  (`spec/static-semantics.md:1355`);
- timers are lost when the process exits, **with the state that scheduled
  them**. That is the bundle's existing durability, not a new loss.

Refusing timers off Workers was considered and rejected: the bundle is what
`bynk test` runs at `unit`, and an agent that can't be compiled for `bundle`
can't be tested there or run in the playground.

**The open dependency.** A timer fired in process has no caller, and on
`bundle` an agent's capabilities come from the caller's `deps`
(`lower_call/agents.rs:27-29`). The test tiers are fine, since the harness
builds the deps. Production bundle use needs the agent composition root that
`agent-capability-encapsulation.md` §3 [DECISION B1] leaves open. Slice 4
depends on that decision (§14).

## 10. Interactions

- **`self.erase()` (#3931).** The schedule is the instance's storage, so erasure
  deletes it with everything else, and the runtime clears the platform alarm
  explicitly rather than relying on `deleteAll()` (§2.4). A timer scheduled in
  the erasing handler is discarded, as #3931 B discards other writes. #3931 D
  already expects this.
- **Events from a timer handler** are released at commit, and the handler gives
  its queue place up for the fan-out, as any handler does (ADR 0781 D5).
- **Active `Cache` eviction.** The runtime schedules an internal sweep, not a
  user timer, at the earliest `Cache` expiry it knows of (the expiry keys of
  `entries.ts:104-109`), coalesced on the same platform alarm. The sweep runs
  as a handler with no body: it reads the first-party `Clock` and deletes expired
  entries through the ADR 0791 commit path, 64 at a time, then re-arms. It
  shares ADR 0791's guarantee that reaping is unobservable. It is on for every
  `Cache` field, with a minimum interval between sweeps (a named constant) so a
  busy `Cache` doesn't wake the object every few milliseconds.
- **Migrations and the schema lock.** A pending timer is stored data, so
  `bynk.schema.lock` records each timer's name and parameter types. An additive
  change to a parameter type is accepted, as for stored values; a non-additive
  one is refused with `bynk.agent.non_additive_state_change`, naming
  `timer expire`. Removing a timer is accepted with a warning
  (`bynk.timer.removed_with_pending`, proposed); a pending entry for a timer
  the build no longer declares is dropped when it falls due and logged by name.
  The lock keeps a **tombstone** for a removed timer's name and parameter types,
  so re-declaring that name later is checked against them: an additive change
  is accepted, and a non-additive one is refused
  (`bynk.timer.redeclared_incompatible`, proposed) rather than turning old
  pending entries into `RehydrationViolation` retries. To change a payload
  non-additively, declare a new timer and keep the old handler until its
  entries have drained.
- **Sagas (#3185).** "A Durable Object with alarms" is one of its substrate
  candidates. This track provides the primitive; it does not choose.
- **The durable-work pattern.** ADR 0122 D4 says agent-local deferred work is a
  `Log` plus a cursor, "drained later (e.g. on an alarm)"
  (`0122-queue-is-a-delivery-concern.md:69-75`). A zero-parameter timer is that
  alarm.

## 11. Limits

- **Pending timers per instance:** a cap, proposed 10,000. A commit that would
  exceed it faults (`TooManyTimers`) and commits nothing. The cap bounds storage
  and identity entries; the earliest-entry read is one bounded `list`, whatever
  the count.
- **Smallest delay:** zero. `after(0.seconds)` runs as soon as the alarm can fire
  after the commit, which makes "do this after I return" expressible. An `at` in
  the past is due at once.
- **Largest delay:** no language limit. Any platform bound is to confirm (§2.4);
  if one exists, the runtime arms in chunks, as on `bundle`.
- **Batch per wake:** proposed 64 timers (§6.1).
- **Clock:** the first-party `Clock` (§5.2).

## 12. Risks

- **At-least-once surprises authors.** A timer handler that sends an email can
  send it twice. → The reference says so where timers are introduced, the
  guides use `Idempotency.claim`, and `fire` makes the duplicate case easy to
  test.
- **A custom `Clock` that disagrees with wall time.** If a context's own
  `provides Clock` runs behind wall time, the platform alarm fires before the
  agent's clock says anything is due, and the dispatch re-arms. → A minimum
  re-arm interval stops a tight loop; the reference notes that timers follow the
  `Clock` the agent is composed with.
- **Payload identity.** Identity compares encoded `Key` values. → Restricting
  parameters to `Key` types (no `Float`, no value `Map`) is what keeps the
  encoding canonical; slice 1 tests that equal values encode equally.
- **Platform facts not yet confirmed** (§2.4). → The design does not depend on
  them (§5.3, §6.3). The slice 0 ADRs cite Cloudflare's documentation for each.
- **Cost.** Every alarm wake is billed as a request to the object. → Timers are
  coalesced on one alarm, the `Cache` sweep has a minimum interval, and a
  spurious wake does one read.
- **`advance` changes shape.** It becomes awaited in modules with timers. → Only
  those modules change; the golden fixtures for the rest stay byte-identical.
- **A key field named `timers`** would make `self.timers` ambiguous. → Refuse it
  in an agent with timers (`bynk.timer.key_named_timers`), the same treatment
  #3931 needs for `erase`.

## 13. Threat model

None of substance, because no new party is admitted. A timer is scheduled only
by the agent's own handlers, for its own instance (§4.2), and runs only from the
instance's own alarm. No request, other agent or other context can schedule,
cancel or fire one: timer handlers are not on the call surface (§4.1), and the
`fire` route exists only in test builds (§8.3). The payload was written by the
agent itself and is read back through the boundary codec like any stored value,
so a corrupt entry faults as `RehydrationViolation` rather than running. Logs
and fault reports name the agent and timer, never the key or payload. If a
later slice proposes scheduling on another instance's reference, it reopens
this section and needs `/security-review`.

## 14. Slices

| # | Slice | Depends on |
|---|---|---|
| 0 | Settle this doc; land the four ADRs (§15) | — |
| 1 | Timers on `workers`: `on timer`, `self.timers`, checker rules, the stored schedule, arming (§5.3), dispatch, retry and abandonment (§6); behavioural tests on the in-memory storage | 0 |
| 2 | Tests: alarms on the harness clock, awaited `advance`, `fire`, `system` tier; ADR 0523 C amended | 1 |
| 3 | Integration: `self.erase()` clears the schedule; the schema lock records timers (§10) | 1, #3931 |
| 4 | `bundle`: the in-process driver (§9) | 1, agent-capability-encapsulation [DECISION B1] |
| 5 | Active `Cache` eviction on the alarm | 1, 2 |
| 6 | The Book: `reference/agents.md` "Timers", `reference/testing.md` time section, spec grammar and emission, guides "Expire an unpaid order" and "Send a reminder" | 1–5 |

Each slice states its tooling (hover on `on timer` and `self.timers`,
completion of timer names after `self.timers.` and after `fire`), per
ADR 0156. Slice 4 is new relative to the spine's candidate list, which folded
the bundle into Q8.

## 15. Decisions (ADRs to land up front)

- **D1. The timer model.** Named, typed timers declared by `on timer`;
  parameters are `Key` types; identity is name plus payload; scheduling an
  identity replaces it; `cancel` is idempotent; scheduling is on `self.timers`,
  for the own instance only.
- **D2. Schedule changes are part of the atomic commit.** They are store writes
  in the handler's transaction; the platform alarm is armed early, never late.
- **D3. At-least-once firing.** A timer's entry is deleted in its handler's
  commit; the runtime owns retries with backoff; after the last attempt it is
  abandoned with a logged and reported `TimerAbandoned`.
- **D4. Timer time is `Clock` time.** Due instants are stamped and judged by the
  agent's `Clock`. Amends ADR 0523 C for agent timers.

Recorded in their slices: the `fire` statement and awaited `advance` (slice 2),
timers in the schema lock (slice 3), the bundle driver (slice 4), the `Cache`
sweep (slice 5).

## 16. Open questions — proposed answers

Proposed for review in the settling PR. None is settled until the maintainer
agrees.

1. **One `on alarm`, or named timers?** *Proposed: named, typed timers
   (`on timer expire(order: OrderId)`).* An agent usually has more than one kind
   of pending work, and each needs a payload. With one `on alarm` every author
   would keep a schedule in a `Map` and dispatch on it by hand, which is exactly
   what the runtime must do anyway for the one platform alarm and for `Cache`
   eviction. *Rejected:* a single handler, as the design notes sketch: simpler
   to declare, but it pushes multiplexing into every program.
2. **Scheduling surface.** *Proposed: members of `self`
   (`self.timers.expire(order).after(30.minutes)`, `.at(t)`, `.cancel()`).*
   Scheduling is a write to the agent's own stored schedule, committed with its
   other writes, so it belongs with `store` operations rather than with
   capabilities. Using `self` makes "own instance only" structural: no
   expression names another instance's timers. The design notes' testability
   argument is met by `advance` and `fire`. *Rejected:* an `Alarms` capability:
   it would be callable from services, need a special rule to bind it to the
   own instance, and imply a stub that replaces committed behaviour.
3. **Identity, replacement and cancellation.** *Proposed: identity is name plus
   payload; scheduling a pending identity replaces it; `cancel()` on the same
   identity is idempotent.* Per-order expiry needs one pending timer per order,
   so the payload must be part of identity, and replacement gives "time out
   twenty minutes after the last touch" for free. Restricting parameters to
   `Key` types keeps identity well defined. *Rejected:* identity by name only,
   which allows one pending `expire` per agent.
4. **Atomicity.** *Proposed: yes; schedule changes are staged in the overlay and
   written in the handler's one transaction, and the platform alarm is armed
   before the flush and corrected after.* A handler that faults then schedules
   and cancels nothing, matching every other write (ADR 0109). Arming early
   means a crash can only cause a harmless spurious wake. *Rejected:* setting
   the alarm only after the commit, which can lose a timer on a crash.
5. **At-least-once firing.** *Proposed: an entry is deleted in its handler's
   commit; the runtime catches faults and retries each timer with its own
   backoff (proposed 30 s doubling, 5 attempts); then it is removed and
   `TimerAbandoned` is logged and reported.* One platform alarm carries many
   timers, so throwing from `alarm()` would re-run all of them under a policy we
   neither set nor test. The platform's retry still covers runtime crashes.
   *Rejected:* relying on the platform's alarm retry, and a stored dead-letter
   list in the first cut.
6. **Queueing.** *Proposed: each due timer takes its own place in the instance's
   queue, with the agent's wait limit; a timer that waits too long stays
   pending, is re-armed, and is not counted as an attempt or a fault.* ADR 0781
   already names alarms as an entry point. Per-timer places stop a backlog from
   locking callers out, and treating a busy wait as load, not failure, follows
   #3932. *Rejected:* one queue place for the whole alarm batch, as the wrapper
   does today.
7. **Tests.** *Proposed: `advance Clock by` fires due timers in due order, each
   awaited, interleaved with the existing timer queue; a `fire
   Agent(key).timer(args)` statement runs one now; at `system` the harness
   drives the real `alarm()` of in-memory objects.* This reuses the harness's
   timer queue and keeps one time source per case. `fire` is the only way to
   test a duplicate delivery. *Rejected:* allowing a plain call to a timer
   handler in a case: it would compile in a test and fail when moved to a
   handler.
8. **`bundle`, Node and the browser.** *Proposed: timers run in process on a
   host timer, with the same stored schedule in the in-memory state; they are
   lost on exit along with that state.* That matches the bundle's existing
   durability and keeps agents with timers testable at `unit` and runnable in
   the playground. Production bundle use waits for the agent composition root
   (§9). *Rejected:* refusing timers off Workers.
9. **Interactions.** *Proposed:* `self.erase()` deletes the schedule and clears
   the alarm; events from a timer handler are released at commit as usual;
   active `Cache` eviction is an internal sweep on the same alarm; the schema
   lock records timer payloads, accepting additive changes, refusing
   non-additive ones, and dropping (with a log line) entries for a removed
   timer; sagas (#3185) may build on this but are not decided here. Each follows
   from treating the schedule as ordinary stored state. *Rejected:* refusing to
   remove a timer that might have pending entries, since no build can know.
10. **Limits.** *Proposed: a cap of 10,000 pending timers per instance
    (`TooManyTimers` at commit); a smallest delay of zero; no language maximum;
    the clock is the first-party `Clock`, not wall-clock time.* The `Clock` choice
    keeps timers deterministic under the harness, for the reason ADR 0791 judges
    `Cache` expiry by the program's clock: a runtime clock disagrees with a fake
    one. On Cloudflare the first-party `Clock` is wall-clock time, so production
    behaviour is the same. *Rejected:* the platform's wall clock (ADR 0523 C as
    it stands), which would make `advance` unable to fire a timer at `system`.
