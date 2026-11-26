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
that subscriber's dead-letter queue after `retries` failed attempts. Duplicates are possible
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
`retryDelay × attempts` (default `retryDelay` 10 seconds), up to `retries` (default 3) times.
After the last failed attempt the platform moves the message to the subscriber's dead-letter
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
`entries.ts` already uses: `m/`, `s/`, `l/`, `i/`, `x/`, `c/` and `"state"`. When the outbox is
non-empty, it also sets the DO alarm about 30 seconds ahead.

- A row is `{ "v": 1, "type", "payload", "envelope", "sent": [] }`: the emission, plus the
  subscriber queues it has already reached.
- **Flush:** after the commit, the agent replies, then flushes in the background. For each
  row, it sends to every queue in the *current* routing table that is not in `sent`, then
  deletes the row; a partial flush updates `sent`. Computing the targets at flush time means a
  row written before a redeploy goes to the subscribers the running build knows.
- **Backstop:** the alarm handler runs the same flush. A DO alarm has at-least-once execution
  and is retried with exponential backoff, up to 6 retries
  ([alarms](https://developers.cloudflare.com/durable-objects/api/alarms/)). The handler
  therefore catches its own failure and sets a new alarm rather than throwing, as Cloudflare
  recommends for work that must not stop
  ([rules of Durable Objects](https://developers.cloudflare.com/durable-objects/best-practices/rules-of-durable-objects/)).
  Backoff runs from 30 seconds up to 1 hour.
- **Duplicates:** a crash between a send and the row's delete resends with the same
  `eventId`; D11 absorbs it.
- **Erasure:** a commit that erases the instance (`self.erase()`) must keep pending outbox rows
  and the alarm. The erase writes them back after its `deleteAll` in the same transaction.
  `bynk migrate sweep` and agent moves (#3819) preserve or flush the outbox before they touch
  storage.
- **One alarm per DO.** No agent uses the DO alarm today: nothing in `bynk-emit` calls
  `setAlarm`. If a user-facing agent alarm lands later, the runtime multiplexes the two
  (store each due time and set the alarm to the earliest), which is the pattern Cloudflare
  documents for several schedules
  ([alarms](https://developers.cloudflare.com/durable-objects/api/alarms/)).
- **Cost on the hot path:** one more `put` per emission inside a transaction the handler
  already pays for. The reply is held only until that write confirms (ADR 0109 D2's output
  gate), not until any send. The live measurement is §9 L3.

**Services.** No outbox. After the body completes, the wrapper sends each emission to its
subscribers' queues, all queues concurrently, one `sendBatch` per queue per handler. A failed
send is retried twice (after 100 ms and 400 ms); if it still fails, the handler faults with
`EventSendFailed`, logged with the event type and the queue.

- A successful `send` is durable: *"messages written to a queue should never be lost once the
  write succeeds"* (§3). The queue write is therefore the durable hop.
- A fault reaches the caller as one (a 5xx for HTTP, `Retry` for a queue handler, a faulted run
  for cron), so nothing is lost silently.
- Some emissions may already have been sent when the handler faults. That is consistent with at
  least once.

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

### D2 (Q2). Protecting the internal queues

**[DECISION] Reserved names refused at compile time, and every message is a #3899 signed
queue message. Subscribers verify through the shared path before they decode. (Recommended.)**

**Names.** A subscriber service's queue is `bynk-ev-<slug>-<hash>` and its dead-letter queue
is `bynk-dl-<slug>-<hash>`:

- `<hash>` is the first 8 hex digits of SHA-256 over `<qualified context>/<service>`, which
  makes the name injective;
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

1. `admitQueueBody(received, batch.queue, [], env, {}, admitsContext)` (`queue_admit.ts`), with
   `admitsContext` admitting exactly one context: the owner of `E`, known at compile time.
   This is the same function `Producer`-refinement consumers use, so there is one signing
   format and one verification path (the input from #3898's 2026-11-22 comment).
2. `body.envelope.publisherId` must equal the verified `from`, and `body.type` must be the
   event the service subscribes to.

So ADR 0289 (owner-only emission) and ADR 0485 (a proven `publisherId`) now hold at the
subscriber by signature, not only by the compiler's check on the emitter.

What a failed check does follows #3899 F:

- unsigned, malformed, another version, a sender that is not the owner, or a bad signature:
  acknowledged, reported as a fault, counted, never handled, never retried or dead-lettered;
- a verified message whose `publisherId` or `type` disagrees with the signer: a compiler or
  deploy bug, not an attack, so it goes to the dead-letter queue (as an undecodable body would,
  D5) and is reported.

**Key rotation.** A message can wait up to 15 days (14 of retention plus 1 of delay), so a key
rotation must keep the previous key that long. This is a **prerequisite, and a gap in shipped
code** (reported to the coordinator as a finding):

- `queueForeignKeys` returns `<SECRET>_PREVIOUS` for `Foreign` actors, but for a Bynk context it
  returns only the current edge key (`[edgeKeyOf(env, EDGE_FROM_PREFIX, from)]`);
- `decide_edge_key` mints a fresh key under `--force`, or after a Worker was deleted, and keeps
  no previous value.

So today a `--force` rotation makes queued `Producer`-refinement messages fail `bad_signature`,
and they are acknowledged and dropped. Slice 1 adds `BYNK_CALL_FROM_<X>_PREVIOUS`. Deploy writes
the replaced key there when it rotates an edge, records the rotation time in the ledger, and
removes it on the first deploy 15 days after. `queueForeignKeys` tries both keys.

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
  listed as breaking in §8.
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

**Why a ruling:** this withdraws behaviour that is documented and measured today, which a
subscriber may rely on, for example an "order created" then "order paid" pair emitted by one
handler. The alternatives are product trade-offs:

- **(i) keep (a)** by sending one handler's emissions to a subscriber as *one* message, an
  array, so their order is intrinsic. The subscriber's generated entry would then iterate, with
  one acknowledgement covering several events. A failure of event 3 of 5 then retries all five,
  and deduplication (D11), which is per `eventId`, skips the two that completed. This is
  feasible, but it couples the events' fates and makes the message size limit apply to the
  sum;
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
  will never decode.
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
  was released when it failed, so D11 admits it once.

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
   subscriber's consumer is attached. Nothing is lost while the queue retains them.

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
  - D1: 2 writes per delivery (the claim, then the confirm), in the subscriber's
    `D1_IDEMPOTENCY` database.
  - Agents: one outbox `put` inside the commit, one delete after the flush, and an alarm only
    when a flush fails.
- **Worked example:** 1,000,000 emissions a month to 2 subscribers each, under 64 KB: 6,000,000
  queue operations, of which 5,000,000 are billed, so **$2.00 a month** for Queues, plus
  4,000,000 D1 rows written.
- **Against today:** each emission is one DO request (the fan-out) plus N Service Binding
  subrequests, with a subscriber invocation each. Every subscriber is invoked, matching or not
  (P6), and all of it is in the publisher's request. The new model moves cost from the
  publisher's latency to background Queue operations, and filtering (D6) removes non-matching
  invocations. Accounting against real traffic is live check §9 L9.
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
- **Keyed on:** `queueReplayKey(<subscriber context>.<service>, <queue>, <eventId>)`
  (`queue_signing.ts`). The key is per subscriber, so two subscribers of one event never
  collide, and per queue, so environments never collide.
- **How:** `queueAdmit`'s existing protocol. It claims after the signature verifies, so
  unsigned traffic cannot fill the store. A 15-minute lease covers the handler's run. The
  claim is **confirmed** for the window when the handler completes, or when the guard excludes
  the event, and **released** when the handler faults, so the retry is admitted. A redelivery
  that finds a lease still running is retried after the lease rather than acknowledged
  (`in_flight`), so nothing is lost.
- **Retention:** `QUEUE_DEDUP_WINDOW_SECONDS`, 15 days: the longest retention (14 days) plus the
  longest delay (24 hours). A message signed longer ago than that is refused as `stale`, so a
  lapsed claim can never let a replay through.
- **Cost:** two D1 writes per delivery (§D9), and every subscriber Worker gets
  `D1_IDEMPOTENCY`, as `Idempotency` users and signed consumers already do.
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
- **Before the push:** both queues of every subscribing service are created if missing (REST,
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
| Up to 1 MiB per delivery | About 127 KB per event, faulting before commit | Yes, for large events |
| Every subscriber invoked | Only matching subscribers (D6) | No (unobservable, except in cost) |
| Publisher↔subscriber call cycle refused | Deploys | No (relaxes) |
| `/_bynk/event/` route | Queue entry; the route kept for one release (D8) | No, within the overlap |
| `Idempotency.claim(env.eventId)` advised | Built in | No (the claim stays correct) |

Source is unchanged: nothing has to be edited. The upgrading note and changelog entry come with
slice 4 (the cut-over).

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
| L5 | Dead-lettering: with `retries: 2`, a faulting subscriber is invoked 3 times with `attempts` 1–3; the message reaches the dead-letter queue unchanged; it still verifies against its source queue; and redriving it by hand runs the handler once. | slice 3 |
| L6 | Provisioning and pruning: creating 2 queues per subscriber in a named environment; whether the queue API reports a backlog count before deletion; deleting a queue with a backlog. | slice 3 |
| L7 | A publisher and a subscriber that calls it deploy in one run from empty, and from a pre-cut-over deployment (the former cycle, P7). | slice 4 |
| L8 | The outbox's backstop: with the queue producer made to fail (a queue deleted out of band), the alarm retries past Cloudflare's 6 attempts by rescheduling, and delivers once the queue is back. | slice 5 |
| L9 | Cost accounting over a day of synthetic traffic: queue operations, D1 rows written and DO requests, against D9's model. | slice 8 |
| L10 | Trace continuity: a subscriber's logs carry the emission's trace id (#3895) through the queue. | slice 3 |

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

**Slice 1: Edge-key rotation keeps the previous key.** Depends on: none. It can land before
slice 0, since it fixes shipped #3899 behaviour.
- `decide_edge_key` writes the replaced key to `BYNK_CALL_FROM_<X>_PREVIOUS` (and the
  `_TO_` side) on rotation, and records the time in the ledger. A deploy 15 days later removes
  it.
- `queueForeignKeys` tries the current key, then the previous one, for a context.

*Done when:* a unit test of `decide_edge_key` covers rotation, keeping and expiry; a runtime
test admits a message under the previous key and refuses one under an unknown key; L1 has
passed; `reference/wire-format.md` and `reference/queue.md` describe the window.

**Slice 2: Reserved queue names.** Depends on: none.
- `bynk.queue.reserved_name` for `bynk-ev-`/`bynk-dl-` in `Queue @name`, `from queue` and
  `deadLetter`, with its explain entry.
- The naming functions in `bynk-project`, with unit tests for injectivity and the 63-character
  budget under the longest environment name the plan admits.

*Done when:* negative fixtures for the three positions pass; `reference/queue.md` lists the
reservation.

**Slice 3: The subscriber's queue entry, and provisioning.** Depends on: 0, 1, 2. No behaviour
changes for users yet: nothing sends to the new queues.
- A `from Events` service's Worker consumes its event queue: verify (`admitQueueBody`), check
  `publisherId`/`type`, claim, decode, guard, run, and acknowledge or retry with
  `retryDelay × attempts`, logging the last attempt.
- Undecodable messages go to the dead-letter queue; refusals are acknowledged and reported.
- `D1_IDEMPOTENCY` is declared for subscriber Workers.
- The `/_bynk/event/` route stays.
- Deploy: the `EventQueue` kind, creation before push, the ledger, `--prune` as §7, and
  environments.
- `system`: `svc.event` through the queue entry, with D10's cases.
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
- The changelog marks it breaking; `upgrading.md` gets §8's table.

*Done when:* the events run fixtures pass on Workers; a fixture with a subscriber that calls
its publisher deploys in the plan tests; no reference to `__EventsFanout` remains outside the
tombstone; L2, L4 and L7 have passed.

**Slice 5: The agent outbox.** Depends on: 4.
- Outbox rows in `commitStored`'s transaction (`o/<seq>`, `v: 1`), the post-reply flush, the
  alarm backstop with self-rescheduling, erase, sweep and move preservation, and the row format
  ADR from 0(e).
- This is where "at least once from a committed emission" becomes true from end to end for
  agents.

*Done when:* runtime tests cover a crash after commit and before the send (the alarm
delivers), a partial flush (only the missing queues are resent), erase with pending rows, and
a row written by an older runtime version; L3 and L8 have passed and their numbers are recorded
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

- `book/concepts/events/understand-events.md` (slices 4 and 8):
  - the "Delivery, concretely, per platform" table: no fan-out DO, at least once, no publisher
    wait, and ordering per D4;
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
- `reference/upgrading.md` and the changelog (slice 4): §8's table.
- `design/bynk-design-notes.md` §7 (slice 0 ADRs): the emit-lowering note's "Queues with
  topic-as-queue routing, or a custom event-fanout DO" fork, now resolved as a queue per
  subscriber.
