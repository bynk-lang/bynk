# Durable event delivery: one compiler-owned queue per subscriber service

- **Status:** Settling. Spine bynk-lang/bynk#3898. This doc also folds in parts 1 and 2 of
  bynk-lang/bynk#3545 (deduplication); parts 3 and 4 of #3545 (ordering, replay and backfill)
  stay out of scope, and §11 says how this design leaves room for them.
- **Theme:** `design/bynk-design-notes.md` §7, *"Delivery is at-least-once via the runtime"*,
  and "Events in depth". The track supersedes [ADR 0284](../decisions/0284-events-fanout-substrate.md)'s
  substrate decision and amends [ADR 0290](../decisions/0290-events-per-publisher-fifo-scope.md).
- **Posture:** Feature track per [ADR 0076](../decisions/0076-feature-track-posture.md). It
  qualifies on all three axes: it spans several increments, its surface is not yet settled
  (§5), and it is a security boundary (§6): owner-only emission
  ([ADR 0289](../decisions/0289-events-owner-only-emission-check.md)) and a proven
  `publisherId` ([ADR 0485](../decisions/0485-per-edge-call-signature.md)) must survive the
  change of substrate.
- **Why now (before 1.0):** moving from at-most-once to at-least-once changes what an existing
  subscriber sees (duplicates, and no ordering promise). `design/bynk-1.0-definition.md` lets
  the events layer evolve *additively* through 1.x, and this change is not additive, so it lands
  before the stability promise. It also lands before the #3055 validation deployments start to
  rely on events.
- **Not a build authorisation.** Each slice in §10 is cut as an ordinary increment proposal, a
  sub-issue of the spine.

Cloudflare facts in this doc are taken from developers.cloudflare.com and carry their URL.
Facts about Bynk are taken from `main` at `09c6e9a5` and carry a `file:symbol` reference.

---

## 1. The problem, from the code

An emission today travels: handler body → `__events` buffer → `deps.__eventsDispatch` after
commit → one fan-out Durable Object per publishing context → one signed HTTP request per
subscriber over a Service Binding → the subscriber's `/_bynk/event/` route.

| # | Defect | Where |
|---|---|---|
| P1 | **A failed hand-off is dropped.** The publisher's dispatch catches every failure of the round trip to the fan-out DO, logs `EventsFanout dispatch failed`, and returns, because the handler has already committed. Nothing is stored. | `bynk-emit/runtime/src/agent.ts:dispatchToEventsFanout` |
| P2 | **A failed subscriber is dropped.** Each delivery is wrapped in `try { await __deliverEvent(…) } catch { log }`, so a subscriber that faults, or an unreachable one, is logged as `EventsFanout delivery failed` and never retried. | `bynk-emit/src/emitter/compose/events.rs:isolated_delivery`, `bynk-emit/src/emitter/events_fanout.rs:deliver_isolated` |
| P3 | **The fan-out DO stores nothing.** Its constructor discards `DurableObjectState`, so an eviction mid-loop loses the rest of the batch (ADR 0290's own trace of the mechanism). | `events_fanout.rs:events_fanout_ctor` |
| P4 | **The publisher waits for every subscriber, one after another.** An agent handler awaits `deps.__eventsDispatch(__events)` after `commitState` and before it returns. The DO awaits each `__deliverEvent` in turn, and `deliverEvent` awaits the subscriber's response. So the publisher's latency includes every subscriber's run time. | `bynk-emit/src/emitter/emit/handlers.rs:events_flush_item`; generated code, e.g. `bynk-tests/tests/fixtures/positive/1384_agent_emits_workers/expected/workers/ledger/handlers.ts` (`bump`); `bynk-emit/runtime/src/boundary.ts:deliverEvent` |
| P5 | **One bottleneck per publisher.** Every emission from a context goes to `binding.idFromName("singleton")`: one DO instance, one thread. | `agent.ts:dispatchToEventsFanout` |
| P6 | **Every subscriber is invoked, matching or not.** The routing table holds every subscriber of an event, and the pattern and `via schema` guards run as the first lines of the subscriber's generated handler (deliver-and-filter). | `events_fanout.rs:event_routes_table`; `bynk-emit/src/emitter/emit/service.rs` (the `via schema` guard) |
| P7 | **A subscriber that calls its publisher can't be deployed.** The publisher's Worker holds a Service Binding to each subscriber (for delivery). A subscriber that also calls the publisher closes a binding cycle, and Cloudflare rejects an upload whose binding target doesn't exist yet. `bynk deploy` refuses the project and names the publisher. | `bynk/src/deploy/gates.rs:ordered`, `bynk/src/deploy/graph.rs:cycle_publishers`; ADR 0475 D2–D3 |
| P8 | **The `/_bynk/event/` route is a public path.** It answers on the subscriber's public address, defended only by the per-edge signature and its 300-second window. | `bynk-emit/src/emitter/workers_entry/events.rs:events_delivery_route` → `workers_entry/call.rs:verify_gate`; `boundary.ts` (`SIGNATURE_WINDOW_SECONDS`) |

The docs say so already: the delivery table in `book/concepts/events/understand-events.md`
("**At most once.**", "Publisher wait (Workers)"), the events guide index, and
`reference/spec/static-semantics.md` §5.7b, all corrected by #3897.

**What already exists to build on** (all on `main`):

- **Queue consumers** with `QueueResult` verdicts, a `delivery { }` policy (`retries`,
  `retryDelay`, `deadLetter`, `batchSize`, `batchWait`, `concurrency`) written into
  `[[queues.consumers]]`, dead-lettering of undecodable bodies through a per-consumer
  `__BYNK_DLQ_<QUEUE>` producer binding, and `QueueDelivery` metadata (#3877, #3878, #3880;
  `reference/queue.md`).
- **Signed queue messages** (#3899, closed): a published format
  (`{ "bynk": { v, from, id, ts, sig }, "body": … }`, HMAC-SHA256 over the source queue's
  name and the canonical body, `bynk-emit/runtime/src/queue_signing.ts:verifyQueueMessage`),
  per-consumer replay deduplication in the `Idempotency` D1 store
  (`runtime/src/queue_admit.ts:queueAdmit`, `queueReplayKey`, `QUEUE_DEDUP_WINDOW_SECONDS` =
  15 days), and `Producer` refinements that admit Bynk contexts signing under their
  per-edge key (`queue_admit.ts:queueForeignKeys`, `admitQueueBody`). A dead-letter consumer
  verifies against its source queue's name.
- **Durable `Idempotency.claim`** (#2840): one `INSERT … ON CONFLICT` per claim in the Worker's
  implicit `D1_IDEMPOTENCY` database, released on failure and confirmed on success
  (`reference/bynk-capabilities.md`, "`claim`: first delivery wins").
- **Per-edge keys** minted by `bynk deploy` (ADR 0485; `bynk/src/deploy/secrets.rs:decide_edge_key`).
- **The `system` tier drives a Worker's `queue` entry** ([ADR 0522](../decisions/0522-cron-and-queue-at-the-system-tier.md)).
- **Queue provisioning:** environment-qualified names (`bynk-project/src/environment.rs:env_qualify`),
  a ledger record per queue and `--prune` of orphans (`bynk/src/deploy/kind/queue.rs`).
- **Atomic agent commit:** `commitStored` writes the `"state"` record and the changed
  per-entry keys in one storage transaction (`bynk-emit/runtime/src/entries.ts:commitStored`;
  ADR 0109 D1–D2).

---

## 2. The end state

When the track retires:

1. **Delivery is at least once from a committed emission.** For an agent, that holds from end
   to end, independent of the caller: an emission committed with the agent's state reaches
   every matching subscriber's queue. For a service, which has no transactional storage, it
   holds from successful completion: a service handler that completes has handed every
   emission to every matching subscriber's queue, and one that cannot hand them off faults
   (§5, D1).
2. **The publisher hands off and returns.** It waits for the queue write (services), or for
   its own storage commit (agents), never for a subscriber.
3. **A subscriber that calls its publisher deploys.** No publisher binds to a subscriber.
4. **No per-publisher bottleneck.** Sends go straight from the emitting isolate (or agent
   instance) to the queues.
5. **Excluded subscribers are never invoked.** The publisher evaluates each subscriber's
   pattern and `via schema` before it sends.
6. **A failing subscriber retries and dead-letters visibly.**
7. **Redelivery is harmless by default.** The subscriber runtime claims each `eventId` per
   subscriber service, so a platform redelivery or a replayed message is acknowledged without
   running the handler again (#3545 part 2).
8. **The event surface is unchanged:** owner-only emission, typed events, the schema registry,
   patterns and `via schema`, the envelope, release-at-commit, `svc.event(…)` at `unit` and
   `integration`, and in-process dispatch on `bundle`. No author declares an event queue.

---

## 3. The substrate

**One compiler-owned Cloudflare Queue per subscribing service, plus one compiler-owned
dead-letter queue for each.** A subscribing service is a `service … from Events(E)` (one event
type per service; `workers_entry/events.rs` finds the service's single `Event` handler).

- The publisher sends one message per emission to the queue of each subscriber whose pattern
  and schema range match (D6).
- Each subscriber's Worker consumes its own queue through the generated `queue` entry, with
  retries, a dead-letter queue and attempt counts (D5).
- Each message is a #3899 signed queue message: the shared format and the shared verification
  path (D2).

**Why this, and not ADR 0284's choice.** ADR 0284 rejected one queue per *event type*, because a
queue shared by N subscribers has one acknowledgement per message, so one subscriber's retry
would redeliver to siblings that had already acknowledged. One queue per *subscriber* doesn't
have that problem: each message has exactly one consumer, so its acknowledgement belongs to that
consumer. ADR 0284's other reasons for a DO were per-publisher ordering (measured as partial in
ADR 0290, and withdrawn by D4) and a future replay log (not built; see §11).

**Alternatives considered and rejected:**

- **Keep the fan-out DO and make it durable** (pending deliveries in its storage, alarm-driven
  retries). It keeps P5 (the single instance) and P7 (Service Bindings from publisher to
  subscriber), and it rebuilds retries, dead-lettering and concurrency that Queues already
  provide and the `queue` entry already handles.
- **One queue per subscribing *context*** (or per Worker), with each message naming its target
  service. Acknowledgement is per message, so this would also isolate subscribers, with fewer
  queues. It is rejected because Cloudflare's consumer settings (`max_retries`, `retry_delay`,
  `max_concurrency`, `dead_letter_queue`) are per queue
  ([configuration](https://developers.cloudflare.com/queues/configuration/configure-queues/)),
  so per-service `delivery { }` (D5) would be impossible, and one slow subscriber's backlog
  would delay its siblings in the same context.
- **One queue per event type:** rejected by ADR 0284, for the reason above.
- **Cloudflare K2,** a durable log with independent subscriptions, which is the natural shape for
  fan-out and for replay ([K2](https://developers.cloudflare.com/k2/)). It is in public beta, on
  Workers Paid only, with 10 GB per account during the beta. Records are acknowledged,
  released and redelivered as a batch, with no per-record acknowledgement, and it has no
  dead-letter queue
  ([delivery guarantees](https://developers.cloudflare.com/k2/reference/delivery-guarantees/)).
  So one failing event would redeliver its whole batch, and Bynk would rebuild dead-lettering.
  Rejected as the delivery substrate for now; it is the leading candidate for #3545 part 4's
  event log (§11).
- **A Workflow per emission.** Durable steps are more than delivery needs, and the cost is
  per instance.

**Cloudflare facts this rests on:**

- At-least-once delivery: *"messages are guaranteed to be delivered at least once, and in rare
  occasions, may be delivered more than once"*
  ([delivery guarantees](https://developers.cloudflare.com/queues/reference/delivery-guarantees/)).
- *"Messages written to a queue should never be lost once the write succeeds"*, and *"Queues
  does not guarantee that messages will be delivered to a consumer in the same order in which
  they are published"* ([how Queues works](https://developers.cloudflare.com/queues/reference/how-queues-works/)).
- *"Each queue can only have one active consumer"* (same page).
- Messages up to 128 KB, measured as 1,000 bytes per KB and including about 100 bytes of
  internal metadata; up to 100 messages or 256 KB per `sendBatch`; 5,000 messages per second
  per queue; retention configurable up to 14 days; delays up to 24 hours; up to 100 retries;
  250 concurrent consumer invocations; 10,000 queues per account by default
  ([limits](https://developers.cloudflare.com/queues/platform/limits/)). Free-plan retention is
  fixed at 24 hours (same page).
- Messages that reach `max_retries` go to the dead-letter queue if one is configured, and are
  otherwise deleted ([batching and retries](https://developers.cloudflare.com/queues/configuration/batching-retries/);
  [dead-letter queues](https://developers.cloudflare.com/queues/configuration/dead-letter-queues/)).

---

## 4. Delivery semantics

### 4.1 The guarantee

> **An emission committed by an agent handler is delivered at least once to every subscriber
> whose pattern and schema range match it. An emission from a service handler that completes
> successfully is delivered at least once in the same way, and a service handler that cannot
> hand its emissions off faults instead of completing.**

"Delivered" means the subscriber's handler runs to completion on it once, or the event reaches
that subscriber's dead-letter queue after `retries` + 1 failed attempts. Duplicates are possible
on the wire. Built-in deduplication (D11) means the handler normally runs to completion once
per subscriber.

What the guarantee is not:

- **Not exactly-once side effects.** As with `Idempotency.claim`, work a handler does before
  it faults can repeat on the retry.
- **Not ordered** (D4).
- **Not across a caller's retry of a service.** A caller that retries a failed service call
  runs the body again and mints new `eventId`s, so subscribers see two distinct emissions.
  That is the caller's retry, not a redelivery.
- **Not durable on `bundle`** (D7).

### 4.2 Release at commit (unchanged in meaning)

ADR 0109 D1: a fault before the commit persists nothing and releases nothing. That holds on
every target. What changes is what "release" means on Workers:

- **Agent:** the handler's emissions are written to the agent's **outbox** in the same storage
  transaction as its state (D1). Release is the commit. The flush to queues happens after the
  reply.
- **Service:** the handler's buffered `__events` (`handlers.rs:events_release_body`) are sent
  to the subscriber queues after the body completes and before the handler returns.

### 4.3 Retries, dead-lettering and visibility

On the subscriber (D5): a handler that faults is retried, with a delay of
`retryDelay × attempts` (default `retryDelay` 10 seconds). `retries` (default 3) counts
*retries*, not attempts: with `retries: N` the handler runs at most N + 1 times. After the last
failed attempt (attempt N + 1) the platform moves the message to the subscriber's dead-letter
queue. The generated entry knows the attempt count (`message.attempts`) and logs
`EventDelivery dead-lettered` with the event type, subscriber, `eventId` and attempts on that
last attempt, so a dead letter shows in the Worker's logs rather than only on the dashboard.
`bynk deploy` prints each subscriber's dead-letter queue.

---

## 5. Decisions

Each question from #3898, then #3545 parts 1–2. **Maintainer ruling needed** marks a product
trade-off with no clear winner. A recommendation is given for every question either way.

### D1 (Q1). The publisher's step

**[DECISION] Agents write an outbox in the commit's transaction. Services send before
returning, retry briefly, and then fault. (Recommended.)**

**Agents.** `commitStored` gains the handler's pending emissions. In the same
`storage.transaction` as the state record (`entries.ts:commitStored`), it writes one outbox
row per emission under a new reserved key prefix, `o/<seq>`. The prefix is outside every prefix
`entries.ts` already uses: `m/`, `s/`, `l/`, `i/`, `x/`, `c/` and `"state"`. When it writes
rows and no alarm is set, it also sets the DO alarm about 30 seconds ahead, in the same
transaction. That alarm is what covers a crash between the commit and the flush. A flush that
empties the outbox **deletes the alarm**, so in the common case the alarm never fires and costs
no invocation. It only fires when a flush failed or never ran. Setting it on every emitting
commit, rather than only on failure, is deliberate: a crash leaves no code running to set it
later. The cost is two DO rows written per emitting commit (`setAlarm` and `deleteAlarm`),
counted in D9.

- A row is `{ "v": 1, "type", "payload", "envelope", "sent": [] }`: the emission, plus the
  subscriber queues it has already reached.
- **Flush:** after the commit, the agent replies, then flushes in the background. For each
  row, it sends to every queue in the *current* routing table that is not in `sent`, then
  deletes the row; a partial flush updates `sent`. Computing the targets at flush time means a
  row written before a redeploy goes to the subscribers the running build knows.
- **Single flight.** While a flush awaits its sends, the DO's input gate is open, so a later
  request's flush or the alarm could list the same `o/` rows. If two flushes ran at once,
  one could update `sent` on a row the other had just deleted and bring it back, causing
  another round of resends. So:
  - each instance runs **at most one flush at a time**. An in-memory `flushing` flag is set
    for the flush's duration. A commit or alarm that finds it set does not flush: it makes
    sure the alarm is set, and the running flush takes the new rows on its next pass, since it
    loops until a listing finds no rows;
  - each row's bookkeeping is **conditional**: the `sent` update and the delete each run in a
    transaction that first reads the row and does nothing if it is gone. A second flush in a
    new isolate, where the flag does not reach, therefore cannot resurrect a row. It can only
    resend, which D11 absorbs.
- **Backstop:** the alarm handler runs the same flush. A DO alarm has at-least-once execution
  and is retried with exponential backoff, up to 6 retries
  ([alarms](https://developers.cloudflare.com/durable-objects/api/alarms/)). The handler
  therefore catches its own failure and sets a new alarm rather than throwing, as Cloudflare
  recommends for work that must not stop
  ([rules of Durable Objects](https://developers.cloudflare.com/durable-objects/best-practices/rules-of-durable-objects/)).
  Backoff runs from 30 seconds up to 1 hour.
- **Duplicates:** a crash between a send and the row's delete resends with the same
  `eventId`; D11 absorbs it.
- **Erasure.** Today `self.erase()` commits as `storage.deleteAll()`
  (`entries.ts:eraseStored`). That call cannot run inside a transaction: the runtime's
  `DurableObjectTransaction` has no `deleteAll`, and `InMemoryStorage.deleteAll` throws inside
  one (`runtime/src/storage.ts`), as the platform does. On the compatibility date Bynk emits
  (`2026-08-11`, in every generated `wrangler.toml`), `deleteAll()` **also deletes the alarm**:
  *"For Workers with a compatibility date of `2026-02-24` or later, `deleteAll()` also deletes
  any active alarm"*
  ([storage API](https://developers.cloudflare.com/durable-objects/api/sqlite-storage-api/#deleteall)).
  So pending rows cannot survive a plain `deleteAll`, and writing them back afterwards is not
  atomic: a crash in between would lose committed emissions. Erasure therefore becomes two
  phases when there is anything to deliver:
  1. **A logical erase, committed atomically.** When the outbox holds rows, or the erasing
     handler emitted, the commit runs one transaction. It writes the handler's own outbox rows
     and an erasure marker, the reserved key `e`, and sets the alarm. `loadState` treats an
     instance with `e` as erased (zero state, as after `deleteAll` today), so the erasure is
     visible from the moment that transaction commits. No state key is read or deleted in it.
  2. **A physical erase, once the outbox is empty.** When a flush deletes the last `o/` row
     and finds `e`, it calls `deleteAll()`. That is atomic on SQLite-backed objects (the same
     page), and clears every remaining key, the marker and the alarm in one step. Between the
     phases the old state keys stay stored but unreadable, and are billed as storage, which
     ends when the outbox drains.

  With an empty outbox and no emissions, `self.erase()` stays the single `deleteAll()` it is
  today. A commit that writes state to an instance carrying `e` (the agent used again before
  the physical erase) first deletes every non-`o/` key except `e` in the same transaction,
  then removes `e`. It is the one path that pays a full delete inside a transaction, and only
  in that window.
- **Sweeps and moves.** `bynk migrate sweep` and agent moves (#3819) preserve or flush the
  outbox before they touch storage, and honour `e`.
- **One alarm per DO.** No agent uses the DO alarm today: nothing in `bynk-emit` calls
  `setAlarm`. If a user-facing agent alarm lands later, the runtime multiplexes the two
  (store each due time and set the alarm to the earliest), which is the pattern Cloudflare
  documents for several schedules
  ([alarms](https://developers.cloudflare.com/durable-objects/api/alarms/)).
- **Cost on the hot path:** one more `put` per emission inside a transaction the handler
  already pays for. The reply is held only until that write confirms (ADR 0109 D2's output
  gate), not until any send. The live measurement is §9 L3.

**Services.** No outbox. A service has no storage transaction, so "commit" for a service means
"the body completed". Emissions keep today's release point.

- **When the send happens.** `Events.emit` in a service only appends to the handler's
  `__events` buffer (`lower_call/platform.rs:lower_events_emit_call`). Nothing is sent at the
  emit call. After the body returns without a fault, the wrapper sends the buffer
  (`handlers.rs:events_release_body`, whose flush moves from `deps.__eventsDispatch` to the
  queue sends), all queues concurrently, with one or more `sendBatch` calls per queue within the limits in D3.
- **A body that faults sends nothing,** as today. Release-at-commit holds for services in the
  sense ADR 0109 gives it.
- **A failed send** is retried twice (after 100 ms and 400 ms). If it still fails, the handler
  faults with `EventSendFailed`, logged with the event type and the queue.
- **What the caller sees when the send fails.** By then the body has done everything else it
  does: KV or D1 writes, outbound `fetch` calls, calls into agents (which have committed), and
  sends on user queues. None of those can be undone, because a service has no transaction to
  roll back. The caller sees a fault (a 5xx for HTTP, `Retry` for a queue handler, a faulted
  run for cron), even though those effects happened. If the caller retries, the body runs again
  and repeats them, with new `eventId`s. Some emissions may also already have been sent to
  some subscriber queues before the failing one. So the guarantee for a service is precise:
  - *a service handler that completes successfully has sent every emission to every matching
    subscriber queue*;
  - *a service handler that faults may have performed any of its other effects and sent any
    subset of its emissions*.

  This is the same position as any service handler that faults after a side effect today. The
  guide states it, and points authors who need effects and events to be atomic to an agent,
  whose outbox gives exactly that.
- A successful `send` is durable: *"messages written to a queue should never be lost once the
  write succeeds"* (§3). The queue write is therefore the durable hop.

Sending at the end of the handler, rather than at each emit call, is the recommendation: a
body that faults half-way sends nothing, which an emit-time send could not promise.

**Rejected:**

- **A per-context outbox Durable Object for services.** The hop to it can fail exactly as a
  queue send can, so it adds a request and a bottleneck (P5 again) and buys no guarantee the
  queue write doesn't already give.
- **At-most-once for service emissions,** documented. The send is cheap to make visible, and two
  guarantees on one surface would make "events" mean different things by the emitter's kind.
- **An agent outbox flushed only by the alarm.** It adds at least the alarm's delay to every
  emission. The inline flush keeps the common case as fast as today's hand-off.
- **An agent that sends before it commits.** That breaks release-at-commit: a commit that then
  fails would have delivered events for state that never existed.
- **A service that sends at each `Events.emit` call.** A body that faults after an emit would
  already have delivered that event, which breaks release-at-commit for services, the
  property that
  `bynk-tests/tests/system/events_behaviour.rs:bundle_events_publish_subscribe_and_abort_suppresses_delivery`
  pins today.

### D2 (Q2). Protecting the internal queues

**[DECISION] Reserved names refused at compile time, and every message is a #3899 signed
queue message. Subscribers verify through the shared path before they decode. (Recommended.)**

**Names.** A subscriber service's queue is `bynk-ev-<slug>-<hash>` and its dead-letter queue
is `bynk-dl-<slug>-<hash>`:

- `<hash>` is the first 8 hex digits of SHA-256 over `<qualified context>/<service>`. That
  makes a collision unlikely, not impossible, and the slug may be truncated, so the names are
  **collision-checked**, not injective. Every name in a project is known when it is compiled,
  so the compiler refuses two subscribing services whose queue names coincide
  (`bynk.event.queue_name_collision`) and names both. The fix is to rename one. Two services
  sharing a name would share a queue, which allows only one consumer;
- `<slug>` is that pair, lower-cased, with every character outside `[a-z0-9]` written `-`, and
  truncated so that the whole name, plus `-<environment>` from `env_qualify`, fits Cloudflare's
  63 characters.

Both prefixes have the same length, so one budget covers both. The deploy plan already refuses
an environment-qualified name that overflows (`reference/queue.md`).

**Compile-time refusal:** `bynk.queue.reserved_name`, an error, when a `Queue @name("…")`
instance, a `from queue("…")` consumer or a `deadLetter: "…"` names a queue starting with
`bynk-ev-` or `bynk-dl-`. No `.bynk` file in this repository uses either prefix today. Only
these two prefixes are reserved, not `bynk-` as a whole, so the reservation breaks nothing
plausible.

**Message format.** The format is #3899's signed queue message, version 1, unchanged:

| Field | Value for an event |
|---|---|
| `bynk.from` | the publishing context's qualified name |
| `bynk.id` | the emission's `eventId` |
| `bynk.ts` | when the publisher signed it (at the flush) |
| `bynk.sig` | HMAC-SHA256 under the publisher→subscriber edge key, over `bynk-queue-v1`, `from`, `id`, `ts`, the **subscriber queue's deployed name**, and the canonical body |
| `body` | `{ "type": "<event>", "payload": …, "envelope": { eventId, publisherId, emittedAt, schemaVersion, trace? } }` |

The edge key is ADR 0485's. Deploy already mints one per publisher→subscriber edge, today for
the fan-out's signed HTTP delivery. The source queue's name is the subscriber's own queue, so a
message moved from one subscriber's queue to another's does not verify, and a dead-lettered
message still verifies against its source.

**Verification, in the subscriber's `queue` entry, before anything is decoded:**

1. `queueAdmit(received, "<context>.<service>", batch.queue, ports)` (`queue_admit.ts`),
   with ports built as `admitQueueBody` builds them: `queueForeignKeys(env, {}, admitsContext)`
   and the D1 claim, confirm and release. `admitsContext` admits exactly one context, the
   owner of `E`, known at compile time. This is the core `admitQueueBody` and
   `Producer`-refinement consumers already run, so there is one signing format and one
   verification path (the input from #3898's 2026-11-22 comment). The entry calls it directly,
   not through `admitQueueBody`, for two reasons. An event queue has no dead-letter *sources*
   to try. And `admitQueueBody` passes the queue's name as `queueAdmit`'s `consumer`, whereas
   the event entry passes the subscribing service's qualified name, which keeps the D11 key
   readable as the subscriber it belongs to.
2. `body.envelope.publisherId` must equal the verified `from`, and `body.type` must be the
   event the service subscribes to.

So ADR 0289 (owner-only emission) and ADR 0485 (a proven `publisherId`) now hold at the
subscriber by signature, not only by the compiler's check on the emitter.

What a failed check does follows #3899 F:

- unsigned, malformed, another version, a sender that is not the owner, or a bad signature:
  acknowledged, reported as a fault, counted, never handled, never retried or dead-lettered;
- a verified message whose `publisherId` or `type` disagrees with the signer: a compiler or
  deploy bug, not an attack. It goes to the dead-letter queue (as an undecodable body would,
  D5) and is reported, and its claim is **released**, so a redrive after the fix is admitted
  (D5).

**Key rotation.** A message can wait up to 15 days (14 of retention plus 1 of delay), so a key
rotation must keep the previous key that long. This is a **prerequisite, and a gap in shipped
code** (reported to the coordinator as a finding):

- `queueForeignKeys` returns `<SECRET>_PREVIOUS` for `Foreign` actors, but for a Bynk context it
  returns only the current edge key (`[edgeKeyOf(env, EDGE_FROM_PREFIX, from)]`);
- `decide_edge_key` mints a fresh key under `--force`, or after a Worker was deleted, and keeps
  no previous value.

So today a `--force` rotation makes queued `Producer`-refinement messages fail `bad_signature`,
and they are acknowledged and dropped. That is filed as bynk-lang/bynk#4526, and this track
depends on it (slice 1): events must not start using edge keys on queues until a rotated key
keeps verifying for the 15-day window.

**Rejected:**

- **A separate event-message format.** It would give two signing inputs and two verifiers for
  one problem. #3898's comment asks for one format, settled with #3899 in view, and #3899
  shipped one that fits events unchanged.
- **Trusting the queue boundary**, with no signature. Any Worker on the account, an event
  subscription or the dashboard can write to a queue (#3898's comment, quoting Cloudflare).
- **A timestamp window as the replay defence**, as `/_bynk/event/` uses today: messages
  legitimately wait for days. Replays are refused by D11's claim.

### D3 (Q3). Message format and the 128 KB limit

**[DECISION] `Events.emit` measures the message before commit and faults over budget, on every
target. The trace travels in the envelope. (Recommended.)**

- The lowering of `Events.emit` (`lower_call/platform.rs:lower_events_emit_call`) computes the
  UTF-8 length of the message the publisher will send: the signed wrapper with its fixed-width
  fields, and a placeholder signature of the real length. Over **127,000 bytes**, the emitting
  handler faults with `EventTooLarge` (the event type and the size), *before* the commit, so
  nothing is persisted and nothing is lost. The budget is Cloudflare's 128 KB, measured at
  1,000 bytes per KB, less a margin for the platform's metadata of about 100 bytes (§3 limits).
  The exact margin is live check §9 L4.
- The check runs on `bundle` too, so a program that passes its tests on Node can't then fault
  on Workers. It is a runtime fault, not a compile-time check, because payload size is dynamic.
- A flush batches each queue's messages into `sendBatch` calls of at most 100 messages and
  256 KB ([limits](https://developers.cloudflare.com/queues/platform/limits/)).
- **This lowers today's cap.** `/_bynk/event/` accepts up to 1 MiB (`boundary.ts:MAX_BODY_BYTES`).
  An event between about 127 KB and 1 MiB that delivers today faults after the cut-over. It is
  breaking. §8.1 says how an affected project finds its large events before the cut-over.
- **Trace (#3895):** `envelope.trace` is carried in the signed body as today, so the subscriber's
  `queue` entry restores the emission's trace. This differs from user queue messages, which
  carry no trace (`reference/queue.md`, "Traces"), because the envelope is the compiler's own.

**Rejected:**

- **Splitting or spilling large events** to R2 or KV with a pointer: it adds a store and a
  lifecycle for a case no user has asked for.
- **Faulting at send time:** for an agent, that is after the commit, so the event would be lost
  silently.

### D4 (Q4). Ordering

**[DECISION] Best effort. ADR 0290's two partial guarantees are withdrawn. (Recommended;
Maintainer ruling needed.)**

Queues promises no order (§3), and the promise doesn't hold even within one `sendBatch` to one
queue. The guarantees ADR 0290 measured are:

- (a) the emissions of one handler arrive in emission order;
- (b) successive, non-overlapping calls to one agent arrive in order.

They held only because one DO looped sequentially; they cannot be kept on queues without
serialising delivery, with consumer `concurrency: 1` plus a reorder buffer on the subscriber.
That costs throughput and reintroduces a bottleneck. The guide states best effort and keeps its
existing advice: *"A subscriber that needs a total order across concurrent publishes must
carry its own sequence number in the event payload."*

**What currently promises the ordering D4 withdraws:**

- `design/decisions/0290-events-per-publisher-fifo-scope.md`: guarantees (a) and (b) as
  measured, accepted at v0.238.1.
- `site/src/content/docs/book/concepts/events/understand-events.md`, the "Delivery, concretely,
  per platform" table, row "Cloudflare Workers": *"Ordering is preserved within one emission
  (everything a single handler invocation emits) and across successive, non-overlapping calls
  to one agent"*. Its closing paragraph, which says order does not hold across *concurrent*
  invocations, implies the rest does.
- `design/bynk-design-notes.md` §7, "**Ordering: per-publisher.** Events emitted by the same
  agent are delivered to each subscriber in emission order" (line 231). The Open-questions
  summary repeats it: *"Events are committed as a service protocol with at-least-once
  delivery, per-publisher ordering, …"* (line 1947). ADR 0290 already narrowed these
  statements in practice, but their text was never amended.
- `design/decisions/0284-events-fanout-substrate.md`: chose the DO partly as *"a natural,
  honest home for per-publisher ordering"*.
- The test `bynk-tests/tests/system/events_ordering_workerd.rs`, which measures (a) and (b).

Nothing else promises event order. `reference/spec/static-semantics.md` §5.7b and the events
guide index say nothing about it; the design notes' *per-sender FIFO* (§12, cross-agent
calls) is about agent calls, not events, and is unaffected. The ruling therefore amends
ADR 0290, the concepts page, the design notes' §7 and its summary, and the measurement test.

**Why a ruling:** this withdraws behaviour that is documented and measured today, which a
subscriber may rely on, for example an "order created" then "order paid" pair emitted by one
handler. The alternatives are product trade-offs:

- **(i) keep (a)** by sending one handler's emissions to a subscriber as *one* message, an
  array, so their order is intrinsic. The subscriber's generated entry would then iterate, with
  one acknowledgement covering several events. A failure of event 3 of 5 then retries all five.
  D11 as designed cannot skip the two that completed: `queueAdmit` claims the *message's*
  `bynk.id`, so a retried array either meets `in_flight`/`replayed` on that one id or re-runs
  every event in it. Skipping completed events needs **new deduplication machinery**: a
  per-event claim loop inside the entry, with its own lease and settle rules. On top of that,
  it couples the events' fates and makes the size limit apply to the sum. This option is
  feasible, but costs a second deduplication design;
- **(ii) keep neither** (recommended): one message per event, uniform semantics, and the
  simplest dead-letter story.

Room for #3545 part 3 is in §11.

### D5 (Q5). What a subscriber's failure means

**[DECISION] Keep `on event … -> Effect[()]`. A fault means retry, then dead-letter, and
success means acknowledge. (Recommended.)**

- **No verdict type.** ADR 0078's agency rule gives a handler a verdict when the outcome is
  independent of success or failure (a queue handler may `Ack` a poison message). An event
  subscriber has no such choice to make: it handles the event or it fails. Adding
  `QueueResult` (for example to back off with `RetryAfter`) can come later and additively, by
  also accepting `-> Effect[QueueResult]` on `on event`.
- **`delivery { }` on `from Events` services:** admitted, with the keys `retries`,
  `retryDelay`, `batchSize`, `batchWait` and `concurrency`, with the same types and ranges as
  `reference/queue.md`'s table, written into the event queue's `[[queues.consumers]]`.
  - `deadLetter` is refused on an events service (`bynk.event.delivery_dead_letter`): every
    event queue has its compiler-owned dead-letter queue, so `bynk.queue.no_dead_letter`
    never applies.
  - Defaults: `retries: 3` (Cloudflare's), `retryDelay: 10.seconds` (Bynk's), and Cloudflare's
    others. The entry retries a fault with `delaySeconds = retryDelay × attempts`, clamped to
    Cloudflare's range, rather than immediately: a subscriber faulting on a downstream outage
    should not burn its retries in a second.
- **Undecodable messages (#3878)** are a verified message whose envelope or payload does not
  decode as the subscriber's types, typically publisher/subscriber skew during a partial
  deploy. They are sent to the dead-letter queue unchanged through the consumer's own
  `__BYNK_DLQ_<QUEUE>` producer binding, then acknowledged and logged, as #3878 already does
  for a `from queue` consumer with a `deadLetter`. They are never retried, since the same bytes
  will never decode. **Their claim is released** (`settle(false)`), as on the fault path, and
  not confirmed. `queueAdmit` has already claimed the `eventId` by the time decoding fails,
  and a confirmed claim would refuse the redrive that R2's recovery relies on as `replayed`
  for 15 days. Dead-lettering is not handling: only a completed handler, or a guard that
  excludes the event, confirms.
- **Attempts:** not exposed to the handler in this track. `QueueDelivery` (#3880) is a
  `bynk.cloudflare` type, and accepting it on a portable `on event` handler would tie the
  source to Workers. The envelope identifies the *emission*, so a per-delivery count does not
  belong in it. If a need appears, the additive answer is a portable `bynk.EventDelivery`
  parameter.
- **Dead-letter consumers are not offered in this track.** Moving dead letters back to their
  queue (redrive) is a form of replay (#3545 part 4). A dead letter can be inspected on the
  dashboard and stays for the queue's retention (a dead-letter queue *"without an active
  consumer will persist for four (4) days before being deleted"*,
  [dead-letter queues](https://developers.cloudflare.com/queues/configuration/dead-letter-queues/)).
  Redriving by hand works: the message still verifies against its source queue, and its claim
  was released when it failed or was dead-lettered as undecodable, so D11 admits it once.
- **How long a dead letter can be redriven.** Two clocks bound it, and the earlier wins:
  - **the dead-letter queue's retention.** Bynk creates every event queue and dead-letter queue
    with the longest retention, 14 days on Workers Paid (24 hours, fixed, on Free;
    [limits](https://developers.cloudflare.com/queues/platform/limits/)), rather than the
    4-day default. The dead-letter queue has no consumer, so its messages persist for its
    retention;
  - **D11's stale window.** A message signed more than 15 days ago is refused as `stale`
    (`queueAdmit`), because by then its claim may have lapsed. `ts` is set when the publisher
    signs, and a redrive by hand cannot re-sign. So a dead letter can be redriven until **15
    days after the publisher signed it**, however long it spent in its source queue first. On
    Workers Paid, that is the binding limit for any message that waited more than a day in
    its source queue.

  The guide states both clocks. A Bynk-run redrive that re-signs at the publisher, and so
  resets `ts`, belongs to #3545 part 4.

**Rejected:** `QueueResult` on every subscriber (a breaking change to every `on event` handler,
for a choice subscribers don't have); a user-declared `deadLetter` (it would reopen D2's
reserved-name boundary).

### D6 (Q6). Filtering at the publisher

**[DECISION] The publisher evaluates each subscriber's pattern and `via schema` before it
sends; the subscriber's handler-side guard stays. (Recommended.)**

- The routing table the publisher's build bakes in (today `events_fanout.rs:event_routes_table`,
  which names every subscriber) gains each subscriber's guard, compiled as a predicate over the
  payload and the envelope's `schemaVersion`. A subscriber the guard excludes gets no message:
  no queue operations and no invocation.
- **The handler-side guard stays** as a defence (a message from a publisher built before a
  pattern changed) and for `svc.event(…)` in tests, which bypasses any publisher.
- **Staleness.** The routing table is fixed at the publisher's build, as it is today for the set
  of subscribers. With filtering, a subscriber deployed alone (`--context`) with a *wider*
  pattern is under-served until its publisher is redeployed. The deploy plan therefore treats
  the publisher's compiled subscriber table, patterns included, as part of what it compares,
  and warns on a `--context` deploy that changes a subscriber's pattern, naming the publishers
  to redeploy.

**Rejected:** subscriber-only filtering (the end state requires that excluded subscribers are
never invoked, and each wasted delivery costs three queue operations); publisher-only filtering
(it removes the defence).

### D7 (Q7). `bundle`, Node and the browser

**[DECISION] In-process dispatch stays, and retries a faulting subscriber in process up to its
`retries`, then logs it as dead-lettered. It is documented as not durable. (Recommended.)**

- The bundle's `__eventsDispatch` (`compose/bundle.rs`, through `isolated_delivery`) runs the
  same per-subscriber wrapper as the Workers entry: the pattern guard, then the handler, and on
  a fault an immediate retry up to `retries`, then the `EventDelivery dead-lettered` log line.
  Deduplication uses the in-memory `Idempotency` provider, so a subscriber written for Workers
  runs the same code path on Node.
- **The difference, documented:** on `bundle` nothing survives a restart. An emission in flight
  when the process dies is lost, and there is no dead-letter store.
- The `EventTooLarge` check (D3) applies on every target.

**Rejected:** no retry on `bundle`. Subscriber code that is only ever run once in development
hides its idempotency bugs until production.

### D8 (Q8). Migration and deploy

**[DECISION] Cut over in one deploy with a one-release overlap. Subscribers keep the legacy
`/_bynk/event/` route for one minor release; publishers tombstone `__EventsFanout`, drop the
publisher→subscriber Service Bindings, and send to queues. The cycle refusal is lifted.
(Recommended.)**

The deploy order follows from the binding graph, which no longer holds publisher→subscriber
edges:

1. **Provision** every event queue and dead-letter queue (§7) before any push.
2. **Push in dependency order.** Without the fan-out bindings, the only edges are call
   bindings, so:
   - a subscriber that only `consumes` its publisher has no edge to it, and pushes in any
     order;
   - a subscriber that *calls* its publisher pushes after it (the former cycle, P7).
3. In the window where a new publisher runs before its subscriber's new version, emissions
   wait in the subscriber's queue, which exists from step 1, and are delivered once the
   subscriber's consumer is attached. That window is short in a whole-project deploy. It is
   bounded by the queue's retention: 14 days on Workers Paid, as Bynk creates event queues
   (D5), but **24 hours on Free**. After that, Cloudflare deletes the messages without trace,
   which is exactly the at-most-once loss this track removes. So a deploy must not leave the
   window open:
   - **A `--context` deploy of a post-cut-over publisher** whose subscribers' deployed
     Workers are still pre-cut-over (no consumer on their event queue) is **refused** by the
     plan. The ledger records each Worker's event-delivery generation at push. The refusal
     names each such subscriber and the command that deploys them. `--force` overrides it with
     a warning that names the retention deadline;
   - the reverse case, an old publisher and a new subscriber, is covered by the overlap below
     and by slice 8's refusal.

**In-flight events at the cut-over.** The fan-out DO stores nothing (P3). What is in flight at
the moment a publisher's new version goes live is a fan-out request mid-loop in the old
version:

- its remaining deliveries go to subscribers over the old bindings;
- the subscribers' new versions still serve `/_bynk/event/` (the overlap), so those deliveries
  are accepted;
- an old-version delivery that fails is lost, exactly as it would be today (at most once). The
  overlap release does not make the old path durable.

**Tombstone.** Each publisher's `__EventsFanout` export goes to `state = "deleted"`, the
mechanism `bynk/src/deploy/config.rs` already reads (`deleted_durable_objects`, state
migrations S3, #2248). Nothing is stored in it, so no data moves. The tombstone is removed after
the overlap release, through the normal tombstone lifecycle. Live check §9 L2.

**Bindings and the cycle refusal.**

- `BindingSource::EventsFanout`, the `___EVENTS_FANOUT` DO binding, and the subscriber Service
  Bindings the fan-out needed all go.
- `cycle_publishers` and the event-delivery paragraph of `gates.rs:ordered`'s refusal are
  deleted. A remaining binding cycle is a true call cycle, which `bynk.context.consumes_cycle`
  already prevents.
- ADR 0475 D2 and D3 are superseded for events: two-phase upload is no longer needed.

**Is it breaking?** Source-compatible: every program compiles unchanged. Its runtime semantics
change, so it is **breaking** and is released as such, before 1.0:

- a subscriber can now see a duplicate (normally absorbed by D11);
- order is no longer promised (D4);
- an emission over about 127 KB now faults its handler (D3);
- the publisher no longer waits for subscribers: P4 is gone, and code that relied on a
  subscriber's side effects being visible when the publisher returns no longer can;
- subscriber Workers now need the `D1_IDEMPOTENCY` database (D11) and queue consumers.

The upgrading note says each of these.

**The legacy route goes** in the release after the cut-over (slice 8). After that, a publisher
older than the cut-over cannot deliver to a current subscriber. That is acceptable for a
project deployed as a whole, and the deploy plan refuses a `--context` deploy that would leave
an old-version publisher of a current subscriber.

### D9 (Q9). Cost and limits

**[DECISION] Accept 2 queues per subscriber service per environment, and the per-delivery cost
below. (Recommended.)**

- **Quota:** 2 queues per subscribing service per environment against 10,000 per account by
  default ([limits](https://developers.cloudflare.com/queues/platform/limits/)), so 5,000
  subscribing services per environment before the limit. Queues above 10,000 cost $0.10 a month
  each on Workers Paid ([pricing](https://developers.cloudflare.com/queues/platform/pricing/)).
  Free-plan accounts have the same 10,000 and 24-hour retention, which is within D11's window.
- **Per emission with N matching subscribers:**
  - Queues: 3 operations per message (write, read, delete) per 64 KB, so `3N` for events
    under 64 KB, plus one read per retry and one write per dead letter
    ([pricing](https://developers.cloudflare.com/queues/platform/pricing/)). The price is
    $0.40 per million operations beyond the 1,000,000 a month included (same page).
  - D1: about 3 rows written per delivery for the claim, its confirm and the sweep of
    expired claims, in the subscriber's `D1_IDEMPOTENCY` database (broken down below).
  - Agents, per emitting commit: one outbox `put` per emission inside the commit, one delete
    per emission after the flush, plus a `setAlarm` in the commit and a `deleteAlarm` after a
    flush that empties the outbox. The alarm *fires*, costing a DO invocation, only when a
    flush failed or never ran.
- **The claim's D1 cost, per delivery** (D11, through `queue_admit.ts:claimQueueMessage`,
  `confirmQueueMessage`):
  - the claim is one `batch` of an upsert of the claim row and a `DELETE` of up to 20 expired
    rows;
  - the confirm is one more upsert;
  - so at least **2 rows written** per delivery, plus the expired rows the sweep deletes, which
    in a steady state equal the claims that lapse, so about **1 more** on average. Deletes count
    as rows written, and an index on the key column adds a written row per write
    ([D1 pricing](https://developers.cloudflare.com/d1/platform/pricing/)). Planning figure:
    **about 3 rows written per delivery** without counting the index, up to 5 with it. Live
    check L9 measures the real figure.
  - D1 on Workers Paid includes 50 million rows written a month, then $1.00 per million. On
    Workers Free it is **100,000 rows written a day**, and queries fail beyond it (same page).
    So a Free-plan project tops out at very roughly 20,000–33,000 event deliveries a day,
    shared with any other `Idempotency` use. The guide states this limit.
- **Worked example:** 1,000,000 emissions a month to 2 subscribers each, under 64 KB, on
  Workers Paid:

  | Item | Volume | Billed | Cost |
  |---|---|---|---|
  | Queue operations (3 per delivery) | 6,000,000 | 5,000,000 beyond the included 1,000,000 | $2.00 |
  | D1 rows written by claims (3–5 per delivery) | 6,000,000–10,000,000 | 0: within the included 50,000,000 | $0.00 |
  | Agent outbox, if the publisher is an agent (about 4 DO rows written per single-emission commit: the row, its delete, `setAlarm` and `deleteAlarm`) | up to 4,000,000 | 0: within Durable Objects' own included 50,000,000 | $0.00 |
  | **Total beyond the plan** | | | **about $2.00 a month** |

  The claims spend 6–10 million of D1's 50 million included rows a month. Durable Object
  storage is billed on the same metric with its own allowance
  ([Workers pricing](https://developers.cloudflare.com/workers/platform/pricing/): *"Each
  `setAlarm()` is billed as a single row written"*, and deletes count as rows written). At
  about 5–8 million emissions a month to 2 subscribers, the claims cross D1's allowance. Each
  further million such emissions then costs about $6–10 in D1 rows, on top of $2.40 in queue
  operations. The claim is the larger cost at volume, which is why D11 records it as the price
  of always-on deduplication.
- **Against today:** each emission is one DO request (the fan-out) plus N Service Binding
  subrequests, with a subscriber invocation each. Every subscriber is invoked, matching or not
  (P6), and all of it is in the publisher's request. The new model moves cost from the
  publisher's latency to background Queue operations, and filtering (D6) removes non-matching
  invocations. Accounting against real traffic is live check §9 L9.
- **The Free plan.** Two daily caps apply, and **the Queues cap binds first**:
  - **Queues:** Workers Free includes *10,000 operations a day*
    ([pricing](https://developers.cloudflare.com/queues/platform/pricing/)). At 3 operations
    per delivery, a Free-plan project delivers **about 3,300 events a day** across all its
    subscribers, fewer with retries (one more read each) and dead letters (one more write).
  - **D1:** always-on deduplication (D11) costs about 3–5 D1 rows written per delivery.
    Workers Free allows 100,000 rows written a day, and from 1 September 2026 D1 queries
    *"will fail when an account exceeds the daily row read or row write limits"* until the
    limit resets at midnight UTC
    ([changelog](https://developers.cloudflare.com/changelog/post/2026-09-01-d1-free-tier-limit-enforcement/)).
    That alone would allow about 20,000–33,000 deliveries a day, so it binds only when
    other `Idempotency` use in the account consumes most of the allowance.

  The planning figure for a Free-plan project with events is therefore **about 3,300
  deliveries a day**. `bynk doctor` and the deploy plan state it.

  **When the Queues cap is hit,** the publisher's `send` fails. Whether it throws, and with
  what error, is live check §9 L11. This hits publishers, not subscribers:
  - a **service** faults with `EventSendFailed` (D1), so its caller sees the fault for the rest
    of the day;
  - an **agent's** outbox keeps the rows (nothing is lost). The flush recognises the limit
    error and moves the alarm to the next 00:00 UTC instead of its normal backoff, so the
    outbox drains after the reset. Rows accumulate in the agent's storage meanwhile;
  - a subscriber's consumer reads are operations too, so a backlog drains only as the next
    day's allowance allows.

  **When the D1 cap is hit:**
  - the claim query fails, so `claimQueueMessage` throws. The entry
    treats a thrown claim as it treats a fault (`queue_admit.ts:claimDatabase` *"fails the
    message loudly rather than admitting it unchecked"*). Left at that, the message would be
    retried after `retryDelay × attempts` and dead-lettered after `retries` + 1 attempts,
    which would take seconds to minutes. A day's overflow would then pour into the
    dead-letter queue, to be redriven by hand.
  - **So the entry recognises the limit error** (D1's documented daily-limit messages) and
    retries with a delay that runs to the next 00:00 UTC, capped at Cloudflare's 24-hour
    maximum. Messages wait out the reset instead of spending their retries in a burst. Each
    such retry still counts as an attempt, so a project that is over the cap every day
    dead-letters after `retries` + 1 days, which is visible, rather than retrying for ever.
    It logs `EventDelivery deferred: D1 daily limit` once per isolate.
  - **Fail closed, not degrade.** The recommendation is that deduplication never degrades to
    admitting unchecked. Admitting without a claim would reopen D2's replay defence: anyone
    with account access could replay a captured message exactly when the account is over its
    limit, and the platform's own duplicates would run twice. Delay is the cost; nothing is
    lost while the queue retains the message, and nothing is handled unchecked. The rejected
    alternative, degrading for platform redeliveries only, cannot be done safely: a replay
    and a redelivery look identical at the entry.
  - **One open risk,** checked live (§9 L11): on Free, queue retention is fixed at 24 hours
    (§3), so a message delayed to midnight may meet its retention first if it already waited
    hours. If L11 shows retention counts from the original send, the delay is capped at what
    the message has left, minus a margin, and the plan warns a Free-plan project with events
    that a day over the cap can lose events.
  - `bynk doctor` and the deploy plan state the Free-plan figure (about 3,300 deliveries a
    day, D1 permitting) for a project with subscribers, so the cap is known before it bites.
- **Throughput:** 5,000 messages per second per queue (§3) bounds one *subscriber*, not one
  publisher, since every subscriber has its own queue. Above that, `send` throws *Too Many
  Requests* ([limits](https://developers.cloudflare.com/queues/platform/limits/)):
  - a service handler faults (D1);
  - an agent's outbox keeps the event and retries.

### D10 (Q10). Testing

**[DECISION] `unit` and `integration` are unchanged; `system` delivers through the subscriber
Worker's `queue` entry. (Recommended.)**

- **`unit` / `integration`:** `svc.event(e)` and `svc.event(e, env)` call the generated
  handler as today (`reference/testing.md`, "Delivering an event"). No queue is involved, and
  no deduplication runs (the case is the test of the handler).
- **`system`:** `svc.event(e)` builds the message the publisher would send (signed as the
  event's owner, with a fresh `eventId` unless `env` is given) and drives the subscriber
  Worker's `queue` entry with a one-message batch, as ADR 0522 does for `q.message(m)`. It
  yields the handler's outcome. New cases:
  - the same `env` delivered twice: the second runs no handler code;
  - a handler that faults: yields a retry, and the case can read the attempt;
  - the last attempt: logs `EventDelivery dead-lettered`;
  - a message signed by another context, or unsigned: refused, never handled.
- **The publisher side** at `system`: an emitting handler's sends are recorded by the harness's
  queue double, so a case can assert what was sent to whom, including filtering (D6) and
  `EventTooLarge` (D3).
- **`bynk dev`:** local delivery across two `wrangler dev` processes is an open risk (§12 R1),
  settled in slice 3 before the cut-over.
- **The ordering harness** (`bynk-tests/tests/system/events_ordering_workerd.rs`, ADR 0290) is retired with
  D4, or kept as a measurement that asserts nothing about order.

### D11 (#3545 part 2). Subscribers ignore repeats of the same `eventId`

**[DECISION] Built in and always on for Workers delivery, not opt-in. (Recommended.)**

- **What is deduplicated:** one emission delivered more than once to one subscriber service,
  whether by a platform redelivery, an outbox resend (D1), or someone with account access
  putting a copy back on the queue.
- **Keyed on:** `queueReplayKey("<subscriber context>.<service>", <queue>, <eventId>)`
  (`queue_signing.ts`). The entry calls `queueAdmit` directly with the service's qualified
  name as `consumer` (D2, step 1), not through `admitQueueBody`, which would pass the queue's
  name there. The key is per subscriber, so two subscribers of one event never collide, and
  per queue, so environments never collide.
- **How:** `queueAdmit`'s existing protocol. It claims after the signature verifies, so
  unsigned traffic cannot fill the store. A 15-minute lease covers the handler's run. The
  claim is **confirmed** for the window when the handler completes, or when the guard excludes
  the event, and **released** when the handler faults, so the retry is admitted. A redelivery
  that finds a lease still running is retried after the lease rather than acknowledged
  (`in_flight`), so nothing is lost.
- **Retention:** `QUEUE_DEDUP_WINDOW_SECONDS`, 15 days: the longest retention (14 days) plus the
  longest delay (24 hours). A message signed longer ago than that is refused as `stale`, so a
  lapsed claim can never let a replay through.
- **Cost:** about 3 D1 rows written per delivery, up to 5 with the key's index (D9's
  breakdown and worked example). Every subscriber Worker gets `D1_IDEMPOTENCY`, as
  `Idempotency` users and signed consumers already do.
- **Author code:** `Idempotency.claim(env.eventId, …)` in a subscriber stays legal and correct,
  but it is redundant for redelivery. The guide's "idempotency idiom" changes from "the habit
  that will matter once delivery is durable" to "built in". `claim` remains the tool for
  deduplicating by a *business* key, which `eventId` is not.

**Rejected:** opt-in, as #3545's text proposed. The replay defence of D2 needs the claim
unconditionally, and opt-in would make the safe behaviour the one an author has to remember.
#3898's 2026-11-22 comment reaches the same conclusion.

### D12 (#3545 part 1). Webhook deliveries deduplicated by delivery id

**[DECISION] No new design here. It is #918's slice 4, and the mechanism already ships.
(Recommended.)**

- A handler can already claim a webhook's delivery id: `Idempotency.claim(deliveryId, 7.days)`,
  released on a 5xx and confirmed otherwise (#918's comment of 2026-11-12; the guide
  "Deduplicate at-least-once deliveries").
- What remains is #918 slice 4: `X-GitHub-Delivery` becomes a declared, typed envelope field,
  so the key is a typed value, not a header string. It depends on #918 slice 1 (the envelope
  clause) and on nothing in this track.
- This track takes no slice for it and records the dependency. The window should be at least
  the provider's redelivery horizon; the guide's existing `7.days` stands.

**Rejected:** a dedicated `@dedup(header = …)` on routes. It is a closed section of the kind
#918 exists to retire.

---

## 6. Threat model

**Assets:**

- owner-only emission: only the declaring context may emit `E` (ADR 0289);
- the authenticity of `EventEnvelope.publisherId` (ADR 0485);
- the subscriber's input contract: the event type, decoded and refinement-checked;
- delivery itself: a subscriber must not be starved, and an event must not be silently lost.

**Who can write to a compiler-owned queue** (from #3898's comment, quoting Cloudflare):

- any Worker on the account with a producer binding;
- Cloudflare products through event subscriptions;
- apparently the dashboard.

No HTTP publish endpoint is documented. **Who can read or delete:** the dashboard previews
bodies, and a token with `queues_read` and `queues_write` can pull and acknowledge, which
deletes them
([pull consumers](https://developers.cloudflare.com/changelog/post/2025-04-17-pull-consumer-limits/)).
The realistic adversary is anyone with access to the account, not the public internet.

**Adversaries and defences:**

| Adversary | Attempt | Refused by |
|---|---|---|
| User code in the project | names an event queue (`Queue @name("bynk-ev-…")`, `from queue`, `deadLetter`) | `bynk.queue.reserved_name` at compile time (D2) |
| User code in a non-owner context | emits `E` | `bynk.event.emit_outside_owner` (ADR 0289), and the subscriber's `admitsContext` admits only the owner's key |
| Another Worker on the account, an event subscription, the dashboard | injects a message | no edge key, so `unsigned`/`bad_signature`: acknowledged, reported, never handled, retried or dead-lettered (D2) |
| A holder of one edge key (a compromised subscriber of the same publisher) | forges `from` as another context | the key is chosen by `from`, and one context's key cannot verify another's name (ADR 0485's injective suffix) |
| An account member with read access | replays a captured message | D11's claim: `replayed`, acknowledged and not handled; older than 15 days: `stale` |
| The same | moves a message to another subscriber's queue | the signature covers the source queue's name |
| The same | copies a staging message into production | edge keys are per environment, and queue names are environment-qualified, so it does not verify |
| A publisher whose deploy is older or newer than the subscriber's | sends a payload of the wrong shape | verified but undecodable: dead-lettered, never handled (D5) |
| A public client | reaches delivery | no path: the queue has no public endpoint, and the legacy `/_bynk/event/` route (still signed) goes in slice 8 |

**Secrets:**

- Edge keys are Worker secrets, set by deploy, never in source or the ledger (ADR 0195 D1),
  with the previous key kept for 15 days on rotation (D2, slice 1).
- No new secret kind is introduced.
- The `D1_IDEMPOTENCY` claims table holds keys (context, service, queue, `eventId`) and expiry
  times, no payloads.

**What this does not protect** (stated in the guide):

- **Confidentiality.** Account members can read event bodies on the dashboard or by pulling.
  Events must not carry secrets, which is the same guidance as `bynk.cloudflare.Queue`'s *"keep
  secrets out of bodies"*.
- **Availability.** An account member can flood a subscriber queue (each injected message is
  cheap to refuse, but costs an invocation and delays real events), or pull and acknowledge
  real ones to delete them. Account access is Cloudflare's to control.
  - A subscriber's `concurrency` (D5) bounds its load.
  - The per-queue throughput limit (§3) bounds a flood.
  - Refusals are counted in the logs, so a flood is visible.
- **Exactly-once side effects** (§4.1).

**The signing that #3899's consumers use** is the one used here. A change to it is one change,
reviewed once, and covered by `/security-review` on every slice that touches signing,
verification or deduplication (slices 1, 3, 4 and 5), per ADR 0076's security-review gate.

---

## 7. Deploy and provisioning

- **Names:** `bynk-ev-<slug>-<hash>` and `bynk-dl-<slug>-<hash>` per subscribing service (D2),
  passed through `env_qualify` (`-<environment>` outside `default`). The functions live in
  `bynk-project/src/environment.rs` beside `env_qualify`, so the plan and the emitted config
  agree by construction, as #2028 requires.
- **Generated config:**
  - the **subscriber's** `wrangler.toml` gains a `[[queues.consumers]]` for its event queue,
    with `dead_letter_queue` set to its dead-letter queue and the `delivery { }` settings, and a
    `__BYNK_DLQ_<QUEUE>` producer binding for undecodable messages (#3878's mechanism);
  - the **publisher's** `wrangler.toml` gains a `[[queues.producers]]` for each subscriber queue
    it may send to, named by a hash of the subscriber pair, and loses the `__EventsFanout`
    binding and export (tombstoned, D8) and the Service Bindings that only delivery needed.
- **Ledger:** a new resource kind, `EventQueue`, in 1D.1's model (`bynk/src/deploy/kind/`).
  It is separate from `Queue`, so the plan shows *"event delivery"*, and so pruning and reuse
  have their own rules. An event queue is always Bynk's: one found already on the account with
  a reserved name is adopted (recorded as created), since no one else may own that name.
- **Before the push:** both queues of every subscribing service are created if missing, with
  the longest message retention the plan allows (14 days on Workers Paid; D5) (REST,
  per the back-half paydown's Q4 boundary). Cloudflare would also auto-create a missing
  dead-letter queue
  ([dead-letter queues](https://developers.cloudflare.com/queues/configuration/dead-letter-queues/)),
  but creating it ourselves keeps it in the ledger.
- **`--prune`:** an event queue whose subscribing service no longer exists is an orphan. Prune
  deletes it and its dead-letter queue only after the plan shows each with its backlog, if the
  API reports one (live check §9 L6). A queue with a backlog needs `--force`, because deleting
  it loses those events.
- **Environments:** each environment has its own queues, ledger entries and edge keys. Nothing
  is shared across environments.
- **Quotas:** §D9. The plan counts the queues it would create against the account's current
  count and warns when that crosses 90% of 10,000.
- **`bynk doctor`:** reports that a project with events now needs Queues on the account, and
  `CLOUDFLARE_API_TOKEN` with Queues permissions for provisioning (the back-half paydown's Q4).

---

## 8. Migration summary

| Before | After | Breaking? |
|---|---|---|
| At most once | At least once (agents end to end; services from completion) | Yes: duplicates are possible, normally absorbed by D11 |
| Ordered within a handler and across non-overlapping agent calls (ADR 0290) | Best effort (D4, ruling needed) | Yes |
| Publisher waits for every subscriber | Publisher waits for the hand-off only | Behavioural |
| Up to 1 MiB per delivery | About 127 KB per event, faulting before commit; warned a release earlier (§8.1) | Yes, for large events |
| Every subscriber invoked | Only matching subscribers (D6) | No (unobservable, except in cost) |
| Publisher↔subscriber call cycle refused | Deploys | No (relaxes) |
| `/_bynk/event/` route | Queue entry; the route kept for one release (D8) | No, within the overlap |
| `Idempotency.claim(env.eventId)` advised | Built in | No (the claim stays correct) |

Source is unchanged: nothing has to be edited. The upgrading note and changelog entry come with
slice 4 (the cut-over).

### 8.1 Finding events that exceed the new cap

The compiler cannot size an event statically: almost every payload holds a `String` or a
collection, whose length is known only at run time. So detection comes in three layers, and
the first ships a release *before* the cap starts to fault.

1. **A measurement before the cut-over (slice 3).** The release that adds the subscriber's
   queue entry also adds D3's size measurement to `Events.emit` on every target. In that
   release it only *warns*: an emission whose message would exceed the budget logs
   `EventNearLimit` with the event type, the size and the budget, once per event type per
   isolate. It also warns from 96,000 bytes, about three quarters of the budget, so an event that is
   growing shows up early. A project upgraded to that release and run normally (or under
   `bynk test`, where the log is visible) finds every event that would fault, by its type,
   from its own traffic.
2. **The fault at the cut-over (slice 4).** From the cut-over release the same measurement
   faults with `EventTooLarge` before commit (D3), so nothing is lost: the handler fails
   visibly, and an agent's state does not commit.
3. **A compile-time warning where a payload is plainly unbounded.**
   `bynk.event.unbounded_payload` is a warning on an `event` declaration whose payload type
   contains `Bytes`, or a `List`, `Map` or `Set` of a non-scalar type, with no refinement that
   bounds its size. That is where large events come from. A payload made of scalars and
   `String`s does not warn: it would warn on nearly every event and train authors to ignore it.
   The warning names the budget and suggests storing the blob (R2, D1) and sending a
   reference. It is a warning, not an error, because a bounded use of such a type is legal.

The upgrading note tells a project to run the slice-3 release and grep its logs for
`EventNearLimit` before taking the cut-over.

---

## 9. What needs a live check on Cloudflare

Lanes never touch an account. The coordinator writes sandbox scripts and the maintainer runs
them. Each check names the slice that needs its result before it lands.

| # | Check | Needed by |
|---|---|---|
| L1 | Edge-key rotation with messages queued: after `--force`, a message signed under the previous key is admitted, and one signed under a key older than that is refused. | slice 1 |
| L2 | `exports.__EventsFanout` set to `state = "deleted"` on a Worker whose class never stored anything, with a fan-out request in flight during the deploy: the push succeeds, and the in-flight request completes or fails cleanly. | slice 4 |
| L3 | **The agent outbox's latency** (#3898 Q1's measurement): p50/p99 of an emitting agent handler with the outbox write in its commit, against today's dispatch, and the time from commit to subscriber invocation, at 1, 10 and 100 emissions per handler. | slice 5 |
| L4 | The largest event that sends: the exact byte budget including the signed wrapper and the platform's metadata, at the boundary of 128 KB, and `sendBatch` at 100 messages and 256 KB. | slice 4 |
| L5 | Dead-lettering: with `retries: 2`, a faulting subscriber is invoked 3 times with `attempts` 1–3; the message reaches the dead-letter queue unchanged; it still verifies against its source queue; and redriving it by hand runs the handler once. The same for a message dead-lettered as undecodable (skew), redriven after the subscriber is fixed: it is admitted (its claim was released) and handled once. | slice 3 |
| L6 | Provisioning and pruning: creating 2 queues per subscriber in a named environment, with a 14-day message retention set at creation (and what the Free plan does with that setting); whether the queue API reports a backlog count before deletion; deleting a queue with a backlog. | slice 3 |
| L7 | A publisher and a subscriber that calls it deploy in one run from empty, and from a pre-cut-over deployment (the former cycle, P7). | slice 4 |
| L8 | The outbox's backstop: with the queue producer made to fail (a queue deleted out of band), the alarm retries past Cloudflare's 6 attempts by rescheduling, and delivers once the queue is back. | slice 5 |
| L9 | Cost accounting over a day of synthetic traffic: queue operations, D1 rows written and DO requests, against D9's model. | slice 8 |
| L10 | Trace continuity: a subscriber's logs carry the emission's trace id (#3895) through the queue. | slice 3 |
| L11 | On the Free plan: what `send`/`sendBatch` does past the 10,000 daily Queues operations (the error, and whether consumer reads also stop), and that an agent's outbox defers to 00:00 UTC and drains after it; a delivery whose claim fails on D1's daily row-write limit is retried with a delay to 00:00 UTC, and is delivered after the reset; whether a 24-hour-retention queue drops a message delayed past its original send plus 24 hours. | slice 3 |

---

## 10. Slice plan

Each slice is one reviewable PR (or a small, named series), cut as a sub-issue of #3898 that
cites this doc. Each slice that touches signing, verification or deduplication runs
`/security-review`.

**Slice 0: Front-loaded ADRs.** Depends on: this doc merging.
- (a) The substrate: one compiler-owned queue and dead-letter queue per subscribing service;
  supersedes ADR 0284, and supersedes ADR 0475 D2–D3 for events.
- (b) The guarantee as worded in §4.1, and release-at-commit by outbox for agents and by
  send-or-fault for services (D1).
- (c) Ordering is best effort; amends ADR 0290 (D4, as ruled).
- (d) Internal queues: reserved names, the #3899 v1 format for events, owner-only
  verification, and built-in deduplication (D2, D11).
- (e) The outbox row format is versioned and part of the 1.0 stored-data promise, beside
  ADR 0538: a 1.x runtime flushes every earlier row version.

*Done when:* the five ADRs have merged with numbers, and the spine's checklist links them.

**Slice 1: Edge-key rotation keeps the previous key.** This is bynk-lang/bynk#4526, filed from
this settling's finding and owned there; this track does not re-specify it. Its requirement as
seen from here: a message signed under an edge key replaced within the last 15 days still
verifies. Depends on: none. It can land before slice 0, since it fixes shipped #3899 behaviour.

*Done when (for this track):* #4526 has closed, and L1 has passed.

**Slice 2: Reserved queue names.** Depends on: none.
- `bynk.queue.reserved_name` for `bynk-ev-`/`bynk-dl-` in `Queue @name`, `from queue` and
  `deadLetter`, with its explain entry.
- The naming functions in `bynk-project`, with unit tests for determinism, the collision check
  (`bynk.event.queue_name_collision`) and the 63-character budget under the longest environment
  name the plan admits.

*Done when:* negative fixtures for the three positions pass; `reference/queue.md` lists the
reservation.

**Slice 3: The subscriber's queue entry, and provisioning.** Depends on: 0, 1, 2. No behaviour
changes for users yet: nothing sends to the new queues.
- A `from Events` service's Worker consumes its event queue: verify (`queueAdmit` called
  directly with the service's qualified name as `consumer`, D2 step 1), check
  `publisherId`/`type`, claim, decode, guard, run, and acknowledge or retry with
  `retryDelay × attempts`, logging the last attempt.
- Undecodable messages go to the dead-letter queue; refusals are acknowledged and reported.
- `D1_IDEMPOTENCY` is declared for subscriber Workers.
- The `/_bynk/event/` route stays.
- Deploy: the `EventQueue` kind, creation before push with 14-day retention (D5), the ledger,
  `--prune` as §7, and environments.
- `system`: `svc.event` through the queue entry, with D10's cases, plus **a redrive after
  skew**: an undecodable message is dead-lettered with its claim released, and the same
  message, delivered again once the subscriber decodes it, runs the handler once.
- The compiler's `bynk.event.queue_name_collision` check (D2).
- The D1 daily-limit deferral: a claim that fails on the limit retries with a delay to 00:00 UTC, failing closed (D9, the Free plan).
- The size measurement in `Events.emit`, warning only (`EventNearLimit`), and the
  `bynk.event.unbounded_payload` warning (§8.1), so projects find their large events a release
  before the cap faults.
- Settles R1 (`bynk dev`).

*Done when:* the `system` cases of D10 pass; plan snapshot tests show the queues for a
two-context project in `default` and in a named environment; L5, L6 and L10 have passed;
`/security-review` is clean.

**Slice 4: The cut-over (publishers send to queues).** Depends on: 3. **Breaking.**
- Workers publishers replace `deps.__eventsDispatch` with signed `sendBatch` calls to each
  subscriber queue. Services retry briefly, then fault with `EventSendFailed` (D1).
- *Agents send after commit* in this slice, still at most once (a failed send is logged). The
  outbox is slice 5, so this slice stays reviewable.
- `EventTooLarge` before commit on every target (D3).
- `__EventsFanout` is tombstoned; `events_fanout.rs`, `dispatchToEventsFanout`,
  `BindingSource::EventsFanout`, the fan-out bindings, `cycle_publishers` and the cycle
  refusal's event paragraph are deleted (D8). The structural guard in
  `xtask/src/delivery_isolation.rs` moves to the new single home.
- The plan's refusal of a `--context` publisher deploy that would leave pre-cut-over
  subscribers without a consumer (D8, step 3), with the ledger's event-delivery generation
  per Worker.
- The changelog marks it breaking; `upgrading.md` gets §8's table, **with its first row
  stated as this release delivers it**: at least once for services (from completion); for
  agents, still at most once (a failed send after commit is logged) until slice 5's outbox
  lands. The concepts page says the same in this release. Slice 5's release note then
  upgrades the agent row to "at least once from a committed emission". No release documents
  a guarantee it does not give.

*Done when:* the events run fixtures pass on Workers; a fixture with a subscriber that calls
its publisher deploys in the plan tests; no reference to `__EventsFanout` remains outside the
tombstone; L2, L4 and L7 have passed.

**Slice 5: The agent outbox.** Depends on: 4.
- Outbox rows in `commitStored`'s transaction (`o/<seq>`, `v: 1`), the post-reply flush with
  its single-flight rule and conditional bookkeeping, the alarm backstop with
  self-rescheduling, the two-phase erase (the `e` marker), sweep and move preservation, and
  the row format ADR from 0(e).
- This is where "at least once from a committed emission" becomes true from end to end for
  agents.

*Done when:* runtime tests cover:
- a crash after commit and before the send (the alarm delivers);
- a partial flush (only the missing queues are resent);
- **two overlapping flushes**: a post-commit flush and the alarm, and two isolates, with no
  row brought back after its delete;
- erase with pending rows: the logical erase is visible at once, the physical `deleteAll`
  runs only after the last row is sent, and a crash between the phases loses no row;
- a row written by an older runtime version.

Also: L3 and L8 have passed and their numbers are recorded
in the ADR.

**Slice 6: Filtering at the publisher.** Depends on: 4.
- Compiled subscriber guards in the publisher's routing table (D6), the handler-side guard
  kept, and the deploy plan's staleness warning.

*Done when:* a fixture shows an excluded subscriber receiving no message at `system` (through
the queue double), and the guard still refuses at `unit`; a plan test shows the warning on a
`--context` pattern change.

**Slice 7: Subscriber delivery settings, and `bundle`.** Depends on: 3 (Workers part) and 4.
- `delivery { }` on `from Events` services, without `deadLetter`
  (`bynk.event.delivery_dead_letter`), written into the event queue's consumer (D5).
- `bundle` in-process retries, deduplication with the in-memory provider, and the
  dead-lettered log line (D7).

*Done when:* fixtures cover every key's emission into `wrangler.toml` and the refused key;
`bundle` run fixtures show a faulting subscriber retried `retries` times; hover and completion
cover `delivery` on an events service.

**Slice 8: The legacy route goes, and the book.** Depends on: 4 having shipped in an earlier
minor release, plus 5, 6 and 7.
- `/_bynk/event/` and `deliverEvent` are deleted; the plan refuses a `--context` deploy that
  would strand a pre-cut-over publisher (D8).
- The events guide's delivery sections are rewritten for the new guarantee, and #3897's
  at-most-once wording is retired (the docs delta, §13).

*Done when:* no generated Worker serves `/_bynk/event/`; every page in §13 is current; L9 has
been recorded; the track is ready to retire.

**Not a slice here:** #3545 part 1, which is #918 slice 4 (D12).

---

## 11. Out of scope, and the room left (#3545 parts 3 and 4)

**Part 3, ordering guarantees.** Nothing in this design forecloses them:

- The envelope can gain an optional `sequence` additively. `deserialiseEventEnvelope` already
  tolerates an absent optional field (`trace`), so older subscribers ignore it.
- An agent's outbox already assigns a per-instance `<seq>` at commit, which is the natural
  source of a per-publisher-instance sequence.
- A subscriber that opts into ordering could then be given a per-key reorder step (consumer
  `concurrency: 1`, or a per-key Durable Object buffer), declared on the subscription, for
  example `from Events(E) ordered by e.orderId`.
- D4's per-message delivery keeps per-event acknowledgement, which such a mode would build on.

**Part 4, replay and backfill.** This needs an event log with retention and an owner (ADR 0287),
which queues are not: an acknowledged message is deleted.

- The seams this design keeps: every event already carries `eventId` and `schemaVersion`.
- A replayed event is recognised by D11's claim within its window, and admitted after it as a
  new delivery, which is what a deliberate backfill wants.
- A backfill can be fed into a subscriber's own event queue as ordinary signed messages, so the
  consumer path does not change.
- Cloudflare K2 (a durable log with independent subscriptions and retention up to 30 days,
  [K2](https://developers.cloudflare.com/k2/)) is the leading candidate for the log once it
  leaves beta.
- Redriving a dead-letter queue is the first, smallest form of replay, and belongs to that
  track.

The actor-declaration constraint carried from #260 ("the actor declaration shape must not rule
out adding replay or ordering hints later") is unaffected. This track adds no actor surface;
delivery hints would go on the subscription, as above.

---

## 12. Risks

- **R1. `bynk dev` across processes.** `bynk dev` runs one `wrangler dev` per context
  (`bynk/src/dev.rs`). Whether local queues deliver across two `wrangler dev` processes through
  the dev registry is not established (Service Bindings do, per
  [Service bindings](https://developers.cloudflare.com/workers/runtime-apis/bindings/service-bindings/)).
  Slice 3 runs this locally before slice 4. The fallbacks, in order:
  1. run all of a project's Workers under one `wrangler dev` with several `-c` configs, which
     Cloudflare marks experimental (same page);
  2. a dev-only delivery path that calls the subscriber's queue entry over the dev registry.
- **R2. Deploy skew.** A partial deploy can leave a publisher and a subscriber disagreeing on a
  type. Decoding failures dead-letter, never handle (D5), and D6's staleness warning covers
  patterns. Schema evolution itself stays the schema registry's job.
- **R3. Queues outage.** A service emitting during a Queues outage faults (D1), which is
  visible, and is the same exposure as any capability call. An agent's outbox absorbs it.
- **R4. D1 as a dependency of every delivery.** A `D1_IDEMPOTENCY` failure makes the claim
  throw, so the message is retried, never handled unchecked (`queue_admit.ts:claimDatabase`
  "fails the message loudly rather than admitting it unchecked").

---

## 13. Docs delta

None of these pages is edited by this settling PR. Each is updated by the slice named.

- `book/concepts/events/understand-events.md` (slices 4, 5 and 8):
  - the "Delivery, concretely, per platform" table: no fan-out DO, no publisher wait, and
    ordering per D4. The guarantee is stated per release: in slice 4, at least once for
    services and still at most once for agents; in slice 5, at least once for agents too;
  - the paragraph on deploy cycles and "move the services the subscriber calls out of the
    publishing context": removed;
  - "The idempotency idiom": built in (D11);
  - the closing "Delivery is at most once today" paragraph;
  - the diagram's "fan-out" node.
- `book/guides/events/index.md` (slice 8): "Delivery is at most once today … on Workers it does
  wait for delivery to finish".
- `reference/spec/static-semantics.md` §5.7b (slices 4 and 8): "Delivery is at most once …
  On Workers the emitting handler waits".
- `reference/bynk-capabilities.md`, "The `Idempotency` capability" (slice 8): "event delivery
  is at most once today, but claiming `env.eventId` will matter once delivery is durable
  (#3898)".
- `reference/queue.md` (slices 1 and 2): the reserved names; the edge-key rotation window under
  "Who may send".
- `reference/wire-format.md` (slices 1 and 4):
  - "Signed queue messages": events use this format, and the previous-key window for Bynk
    contexts;
  - "What the format does not cover … events": events' messages are now in the published
    format.
- `reference/testing.md`, "Delivering an event" (slice 3): `system`-tier behaviour.
- `reference/tooling/emission.md`, `reference/tooling/bynk-cli.md` (slices 3 and 4): the
  `__EventsFanout` mentions, the event queues in the plan, and the cycle refusal.
- `book/guides/effects-and-capabilities/deduplicate-at-least-once-deliveries.md` (slice 8):
  events need no claim of `eventId`.
- `reference/upgrading.md` and the changelog (slices 4 and 5): §8's table, its agent row
  as each release delivers it (slice 4 note above).
- **The new language surface and diagnostics:**
  - `reference/spec/static-semantics.md`, the `delivery { }` paragraph of the queue rules
    (where `bynk.queue.delivery_bad_option` and `bynk.queue.delivery_not_queue` are defined
    today) (slice 7):
    - `delivery { }` is also admitted on a `from Events` service, with the keys of D5;
    - `deadLetter` there is `bynk.event.delivery_dead_letter`;
    - `bynk.queue.delivery_not_queue` no longer applies to such a service.
  - §5.7b Events, same page (slices 2, 3 and 4): `bynk.queue.reserved_name` (slice 2);
    `bynk.event.queue_name_collision` and `bynk.event.unbounded_payload` (slice 3); and the
    runtime faults `EventTooLarge` and `EventSendFailed`, and the `EventNearLimit` warning
    (slices 3 and 4).
  - `book/guides/events/index.md` and `book/concepts/events/understand-events.md`
    (slices 3, 4 and 7): a "Delivery settings" section for `delivery { }` on a subscriber; the
    size budget, with what to do about a large payload; the dead-letter queue and how long a
    dead letter can be redriven (D5); the Free-plan figure (D9).
  - `reference/queue.md`, "Delivery policy" (slice 7): a note that event subscribers take the
    same section, without `deadLetter`.
  - The generated diagnostic index (`/reference/diagnostics/`, built from the compiler's
    registry and drift-tested, `reference/spec/diagnostics.md`) picks each new code up from
    the registry. Each slice adds its codes' explain entries: `reserved_name` (2);
    `queue_name_collision` and `unbounded_payload` (3); `delivery_dead_letter` (7).
  - Tooling surfaces per ADR 0156, in slice 7: hover and completion for `delivery { }` on an
    events service; a quick fix that removes a refused `deadLetter`.
- `design/bynk-design-notes.md` §7 (slice 0 ADRs): the emit-lowering note's "Queues with
  topic-as-queue routing, or a custom event-fanout DO" fork, now resolved as a queue per
  subscriber; and, as D4 is ruled, "**Ordering: per-publisher.**" (line 231) and the
  Open-questions summary's "per-publisher ordering" (line 1947).
- `book/concepts/events/understand-events.md`'s ordering sentence in the Workers row, as D4 is
  ruled (slice 4).
