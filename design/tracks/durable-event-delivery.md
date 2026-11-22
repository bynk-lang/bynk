# Durable event delivery — one compiler-owned queue per subscriber

- **Status:** Settling in the draft PR that adds this doc. Spine issue: #3898,
  which carries the live slice status.
- **Realises:** `design/bynk-design-notes.md` §7, *"Delivery is at-least-once
  via the runtime"* (line 156), and "Events in depth" (line 162). It supersedes
  the substrate decision of
  [ADR 0284](../decisions/0284-events-fanout-substrate.md).
- **Posture:** Feature track per
  [ADR 0076](../decisions/0076-feature-track-posture.md). It qualifies on all
  three axes (§1.2).
- **Governing records:**
  [ADR 0284](../decisions/0284-events-fanout-substrate.md) (the fan-out Durable
  Object, superseded here),
  [ADR 0288](../decisions/0288-events-slice0-fanout-implementation.md) (its
  concrete shape),
  [ADR 0289](../decisions/0289-events-owner-only-emission-check.md)
  (owner-only emission),
  [ADR 0290](../decisions/0290-events-per-publisher-fifo-scope.md) (ordering,
  amended here),
  [ADR 0109](../decisions/0109-handler-atomic-commit.md) (release at commit),
  [ADR 0475](../decisions/0475-service-binding-only-when-called.md) (bindings
  only when called; the cycle refusal lifted here),
  [ADR 0485](../decisions/0485-per-edge-call-signature.md) (per-edge keys),
  [ADR 0078](../decisions/0078-queueresult-typed-verdict.md) (the agency rule),
  [ADR 0522](../decisions/0522-cron-and-queue-at-the-system-tier.md) (queues
  at the `system` tier),
  [ADR 0557](../decisions/0557-agent-class-rename-and-removal.md) (`exports`
  tombstones). Four ADRs land up front (§13).
- **Related increments:** #3877 (`delivery { }`), #3878 (undecodable
  messages), #3880 (`QueueDelivery`), #3899 (signed queue messages), #3895
  (`EventEnvelope.trace`). #3897 corrects today's docs ahead of this track.

## 1. The theme

### 1.1 What this track settles

Bynk's events (`event`, `Events.emit[E]`, `from Events(E)`) give a context a
typed, owner-only, schema-versioned publish and subscribe. The surface is
sound. Delivery on Workers is weaker than the surface suggests (§2): it is at
most once, the publisher waits for every subscriber, some topologies cannot
deploy, and one Durable Object instance carries every emission of a context.

This track replaces the delivery mechanism and keeps the surface. It settles:

1. **The substrate.** One compiler-owned Cloudflare Queue per subscriber
   service, plus a dead-letter queue for each. The publisher sends each
   emission to the queue of every subscriber whose pattern and schema range
   match. Each subscriber consumes its own queue, with its own retries and
   acknowledgement.
2. **The guarantee.** At least once from a committed emission, and exactly
   where that holds (agents, services, `bundle`).
3. **Trust.** Who may write to a subscriber queue, and how the subscriber
   knows a message came from the publisher.
4. **Ordering.** Best effort, stated plainly, withdrawing ADR 0290's partial
   guarantees.

Everything the author sees stays: owner-only emission, typed events, the
schema registry, patterns and `via schema`, the envelope, release at commit,
and in-process dispatch on `bundle`. No author declares a queue.

### 1.2 Why a track (the ADR 0076 trigger)

- **It spans many increments.** A new delivery path, a publisher-side outbox,
  publisher-side filtering, subscriber delivery settings, a migration off the
  fan-out Durable Object, and new deploy rules. No single proposal could carry
  that.
- **Its surface is not settled.** The publisher's step for services, whether a
  subscriber needs a verdict type, the message format against the 128 KB cap,
  and the ordering statement are all open (§14).
- **It is a security boundary.** Owner-only emission (ADR 0289) and a proven
  `publisherId` (ADR 0485) hold today. A queue that any Worker on the account,
  or a user `Queue @name`, could write to would silently undo both (§11).

## 2. What's true today

### 2.1 The publisher's step

- An agent handler commits its state, then awaits the event flush. In fixture
  `1384_agent_emits_workers`
  (`bynk-tests/tests/fixtures/positive/1384_agent_emits_workers/expected/workers/ledger/handlers.ts:73-74`):
  `await this.commitState(__state);` then
  `if (__events.length > 0) { await deps.__eventsDispatch(__events); }`. The
  flush is emitted by `events_flush_item`
  (`bynk-emit/src/emitter/emit.rs:7210`).
- The commit itself is one storage transaction: `EntryOverlay.flush`
  (`bynk-emit/runtime/src/entries.ts:212-223`) writes every staged put and
  delete, plus any `extra` entries, inside `storage.transaction`. That is the
  hook an outbox would use.
- `dispatchToEventsFanout` (`bynk-emit/runtime/src/agent.ts:236-263`) sends the
  batch to the context's fan-out Durable Object, always the same instance
  (`idFromName("singleton")`, line 251). A non-`ok` response or a thrown error
  is logged and swallowed (lines 257-261). Nothing is stored or retried.
- A service handler gets the same `__eventsDispatch` from its Worker's
  composition (`bynk-emit/src/emitter/workers.rs:483-506`). Services have no
  transactional storage.

### 2.2 The fan-out

- The fan-out class `__EventsFanout` (`bynk-emit/src/emitter/wrangler.rs:90`)
  is generated by `bynk-emit/src/emitter/events_fanout.rs`. Its routing table
  is a compile-time literal (ADR 0288 B).
- It loops over events and subscribers and awaits each delivery in turn
  (`…/1384_agent_emits_workers/…` is a single-subscriber case; the
  two-subscriber fixture
  `bynk-tests/tests/fixtures/positive/2027_event_payload_commons_fields/expected/workers/ev-emitter/events_fanout.ts:26-37`
  shows the nested `for … await __deliverEvent` loop). A failure is caught and
  logged as `EventsFanout delivery failed` (`events_fanout.rs:325-421`,
  `deliver_isolated`). The constructor ignores its storage (`_state`, line 20
  of the fixture).
- It returns `204` only after the loop ends, so the publisher's awaited
  `stub.fetch` includes every subscriber's run time, one after another.
- `deliverEvent` (`bynk-emit/runtime/src/boundary.ts:586-611`) posts
  `{ payload, envelope }` to `/_bynk/event/<service>` over a Service Binding,
  signed under the per-edge key (`boundary.ts:264-282`, ADR 0485).

### 2.3 The subscriber

- The `/_bynk/event/` route (`bynk-emit/src/emitter/workers_entry.rs:1600-1705`)
  verifies the signature first, decodes the envelope and the payload, and
  answers `400` on a decode failure.
- Filtering is deliver-and-filter: every subscriber of `E` receives every
  emission and runs its pattern and `via schema` guard on entry
  (`emit.rs:7193`, `return_undefined_unless`; spec
  `site/src/content/docs/book/spec/static-semantics.md:2392`).
- A handler returns `Effect[()]` and nothing receives its result
  (`static-semantics.md`, §5.7b).

### 2.4 Deploy

- A publisher binds to each subscriber so the fan-out can deliver (ADR 0288 C).
  A subscriber that also calls its publisher is a binding cycle, and
  `bynk deploy` refuses it, naming the publisher (`bynk/src/deploy/graph.rs:16-44`,
  `cycle_publishers`; ADR 0475 D2). The guide documents the limit
  (`guides/events/understand-events.md:486-493`).
- Queues are created before the push and environment-qualified with
  `env_qualify` (`bynk/src/deploy/provisioning.rs:245-270`,
  `bynk/src/deploy/config.rs:1336`). Names follow `is_queue_name`
  (`bynk-check/src/project_model.rs:1860`): 1 to 63 lowercase letters, digits
  or `-`. The suffix counts towards the 63 and is not checked.
- A consumer gets `[[queues.consumers]]` with a fixed `max_batch_size = 10`
  (`bynk-emit/src/emitter/wrangler.rs:523-530`).
- A Durable Object class is removed with a `state = "deleted"` export
  tombstone (`wrangler.rs:174-205`, `ClassTombstones`; ADR 0557).

### 2.5 What the docs say

The spec says *"Delivery is at-least-once and fire-and-forget"*
(`static-semantics.md:2349`), and the guide says the same
(`understand-events.md:148`). Neither is true on Workers. #3897 corrects both
ahead of this track. The guide's per-platform table
(`understand-events.md:482`) states ADR 0290's ordering scope.

### 2.6 Nothing uses alarms yet

The agent class wrapper already serialises an `alarm` entry point
(`bynk-emit/runtime/src/agent.ts:665`, `ENTRY_POINTS`), but nothing in the
emitter or the runtime calls `setAlarm`. An agent's alarm is free for the
outbox to use (see Risks, §10).

### 2.7 Testing

`svc.event(e)` delivers in process at `unit` and `integration`, running the
guard. It is refused at `system` (`bynk.test.event_needs_in_process`,
`static-semantics.md:2701`). Queue handlers run at `system` through the
Worker's `queue` entry (ADR 0522).

## 3. Goals and non-goals

**Goals**

- Delivery is at least once from a committed agent emission, end to end.
- A service emission is either handed off durably or the handler faults. It is
  never lost silently.
- The publisher waits for the hand-off only, never for a subscriber.
- A subscriber that calls its publisher deploys.
- No per-publisher bottleneck.
- An excluded subscriber is never invoked.
- A failing subscriber retries, then dead-letters, visibly.
- Only the publisher can put a message a subscriber will handle on its queue.
- The event surface and the `bundle` target are unchanged.

**Non-goals**

- Exactly-once delivery. Duplicates are made harmless (§7.4), not impossible.
- Ordering guarantees (§14, Q4).
- Replay from a durable log. Out of scope; a later track may build one.
- Consuming a dead-letter queue from Bynk source. A follow-up (§14, Q5).
- Cross-account events.
- Confidentiality of event bodies against account members (§11).

## 4. The surface

Nothing changes for the publisher.

```bynk
context billing

exports transparent { InvoicePaid }

event InvoicePaid = {
	invoiceId: String,
	amount: Int,
}

agent Invoice {
	key id: String
	store paid: Cell[Bool] = false

	on call pay() -> Effect[()] given Events {
		paid := true
		do Events.emit[InvoicePaid](InvoicePaid { invoiceId: id, amount: 100 })
	}
}
```

A subscriber may add a `delivery { }` block (#3877) to tune its queue. Every
key is optional. `deadLetter` is not accepted on an event subscriber: the
compiler always provisions one (§14, Q5).

```bynk
context mailer

consumes billing
consumes bynk { Logger }

service OnInvoicePaid from Events(InvoicePaid) {
	delivery {
		retries: 5,
		retryDelay: 30.seconds,
		concurrency: 4,
	}

	on event(e: InvoicePaid, env: EventEnvelope) -> Effect[()] given Logger {
		Logger.info(e.invoiceId)
	}
}
```

A subscriber that wants the attempt count names an optional parameter of a
new, platform-neutral type (§14, Q5):

```bynk
service OnInvoicePaidAudited from Events(InvoicePaid) {
	on event(e: InvoicePaid, delivery: EventDelivery) -> Effect[()] given Logger {
		if delivery.attempts > 3 {
			let _ <- Logger.warn("audit: attempt \(delivery.attempts)")
		}
		Logger.info(e.invoiceId)
	}
}
```

A subscriber that calls its publisher now deploys:

```bynk
context mailer

consumes billing

service OnInvoicePaidRefresh from Events(InvoicePaid) {
	on event(e: InvoicePaid) -> Effect[()] {
		let _ <- billing.refresh(e.invoiceId)
		Effect.pure(())
	}
}
```

New diagnostics (names provisional, settled in their slices):

- a `Queue @name` or `from queue("…")` naming a reserved internal queue
  (§7.1);
- `deadLetter` in the `delivery { }` of a `from Events` service;
- an emission whose encoded message exceeds the cap is a runtime fault, not a
  diagnostic (§7.3).

## 5. Internal architecture

### 5.1 Shape

```text
publisher Worker                         subscriber Worker (mailer)
───────────────                          ──────────────────────────
handler body
  Events.emit → __events (buffer)
commit:
  agent:   state + outbox, one txn ──┐
  service: (no storage)              │
hand-off:                            │
  for each subscriber whose          │
  pattern and schema match:          ▼
    sign, sendBatch ──────────► bynk-ev-mailer-oninvoicepaid-…  ─► queue entry
                                       │ max_retries                verify sig
                                       ▼                            dedup eventId
                                bynk-ev-…-dlq                       decode, guard
                                                                    on event(…)
```

### 5.2 Publisher side

- **Routing table.** The compile-time literal of ADR 0288 B moves from the
  fan-out class into the publisher's generated code. Each row names the
  subscriber's queue binding, the subscriber's qualified context (for the
  key) and a generated predicate for its pattern and schema range (§14, Q6).
- **Bindings.** The publisher gets one `[[queues.producers]]` binding per
  subscriber queue, under a reserved binding name (for example
  `__BYNK_EV_<n>`). It gets no Service Binding to the subscriber unless it
  calls it (ADR 0475 D1).
- **Hand-off.** For each subscriber, the matching events of one handler's
  batch are signed and sent with one `sendBatch`, chunked at the platform's
  batch caps (100 messages or 256 KB, per
  `reference/bynk-capabilities.md:755`). A subscriber's chunk is independent
  of its siblings'.
- **Agents: the outbox.** At commit, each pending `(subscriber, message)` is
  written into the same `storage.transaction` as the state
  (`entries.ts:212-223`'s `extra` entries), under a reserved key prefix, and
  an alarm is set. After commit the agent tries the hand-off at once and
  deletes the entries it sent. The alarm sends whatever is left and re-arms
  while anything remains (§14, Q1).
- **Services: send or fault.** After the body completes, the hand-off is
  attempted with a short, bounded retry. If it still fails, the handler
  faults: the caller sees a failure and its own retry is the recovery
  (§14, Q1).

### 5.3 Subscriber side

The `queue` entry gains an arm per subscriber queue. For each message:

1. **Verify** the signature (§7.2). Failure: ack, report a fault, count, never
   handle, never dead-letter.
2. **Deduplicate** on `eventId` per subscriber (§7.4). A repeat: ack and log.
3. **Decode** envelope and payload. Failure on a verified message: send to the
   dead-letter queue at once, then ack (#3878 B).
4. **Guard** with the pattern and `via schema`, as today. A miss: ack.
5. **Run** the handler. Success: ack. Fault: retry, until `retries`, then the
   platform moves it to the dead-letter queue.

The `/_bynk/event/` route stays for one release for the migration (§8), then
goes.

### 5.4 `bundle`, Node and the browser

In-process dispatch is unchanged (`emit.rs:3274-3281`): the composition calls
each subscriber directly. No queue, no outbox, no retry (§14, Q7).

## 6. Runtime behaviour on Cloudflare

Platform facts this design relies on. Items marked *to confirm* are checked
against Cloudflare's documentation, or measured, in Slice 0 before the ADRs
are written.

- **Queues deliver at least once**, and may deliver a message more than once
  (quoted in #3880). Ordering is best effort.
- **A message carries only a body.** There are no headers (#3899), so a
  signature travels inside the body.
- **Message cap 128 KB; batch cap 100 messages or 256 KB**
  (`reference/bynk-capabilities.md:755`). Whether 128 KB means 128,000 or
  131,072 bytes, and whether it is measured on the JSON text or on the stored
  form: *to confirm*.
- **Consumer settings:** `max_retries` defaults to 3; without a dead-letter
  queue a message that reaches the limit is deleted; `retry_delay` and
  `max_concurrency` are settable (table in #3877).
- **Retention:** 4 days by default, 14 at most; a send may be delayed. The
  book says a delay is at most 12 hours (`bynk-capabilities.md:743`); #3877
  and the spine's comment say 24 hours. The dedup window depends on this:
  *to confirm*.
- **Who can write:** any Worker on the account with a producer binding;
  Cloudflare products through event subscriptions; apparently the dashboard.
  No HTTP publish endpoint is documented (spine comment).
- **Account queue limit:** the spine gives 10,000 by default: *to confirm*.
- **Durable Object alarms:** one pending alarm per object; an alarm handler
  that throws is retried by the platform. The retry count and backoff, and
  whether `setAlarm` inside `storage.transaction` commits atomically with the
  writes: *to confirm*.
- **Durable Object value size** for an outbox entry on SQLite-backed storage:
  *to confirm*. One entry per message keeps each under the 128 KB message cap
  in any case.

## 7. Trust, format and limits

### 7.1 Reserved names

- Subscriber queues are named `bynk-ev-<readable>-<digest>`, where
  `<readable>` is the subscriber's context and service, lower-cased with
  every non-alphanumeric run written `-`, truncated so the whole name, the
  `-dlq` suffix and the longest `-<env>` suffix fit 63 characters, and
  `<digest>` is the first 8 hex digits of SHA-256 over `<context>/<service>`.
  The digest keeps the name injective after truncation and folding.
- Each subscriber queue has a dead-letter queue, `<name>-dlq`.
- Deploy environment-qualifies both with `env_qualify`, creates them before
  the push (as `provisioning.rs:245-270` does), and records them in the
  ledger as created by Bynk.
- A user `Queue @name("bynk-ev-…")`, `from queue("bynk-ev-…")` or a
  `deadLetter: "bynk-ev-…"` is a compile error. This stops injection from
  inside the project.

### 7.2 The signed message

Shared with #3899 (D). One format, one signer, one verifier:

```text
{
  "bynk": { "v": 1, "from": "<publisher context>", "id": "<eventId>",
            "ts": <seconds>, "sig": "<hex>" },
  "body": "<JSON text of { type, payload, envelope }>"
}
```

- The HMAC-SHA256 key is the publisher→subscriber edge key of ADR 0485
  (`BYNK_CALL_TO_<SUB>` at the publisher, `BYNK_CALL_FROM_<PUB>` at the
  subscriber). Deploy mints it for every publisher→subscriber pair, even
  though the pair no longer has a Service Binding.
- The signed input is one field per line: a scheme tag distinct from
  `BYNK-CALL-1` (so a call signature never verifies as a message), `v`,
  `from`, `id`, `ts`, the **destination queue's physical name**, and the
  SHA-256 of `body`.
- Signing the queue name stops a message being moved between subscribers or
  between environments.
- `body` is carried as text so the signature covers exact bytes and needs no
  JSON canonicalisation. #3899 says "canonical body"; the two increments
  settle on one choice (§14, Q2).
- Key rotation accepts the previous key for the retention window plus the
  maximum delay, as `SignedCookie`'s `previous` does.

### 7.3 Size

The publisher encodes each message in the handler, before commit. Every part
of the wrapper is known then: `from` and the queue name are compile-time
constants, `sig` and `ts` are fixed width. An emission whose largest message
would exceed the cap faults the handler before commit. Nothing is written,
nothing is sent, and the caller sees the fault. `EventEnvelope.trace` (#3895)
is part of the envelope, so it rides inside `body` and counts towards the cap.

### 7.4 Deduplication

The subscriber runtime claims `eventId` per subscriber service in the
`Idempotency` store before decoding the payload, for at least the retention
period plus the maximum delay. It is settled like an author's `claim`:
released if the handler faults (so the retry runs), kept if it succeeds. This
rejects replays by anyone who can read the queue and makes the platform's own
duplicates harmless. On Cloudflare the store is the D1 database a Worker
already gets for `Idempotency` (`reference/bynk-capabilities.md:83-86`); each
delivery costs one round trip to it.

An author's own `Idempotency.claim(env.eventId, …)` stays legal and becomes
redundant for redelivery. The guide says so.

## 8. Migration and deploy

One `bynk deploy` moves a project across:

1. **Create queues.** Every subscriber queue and dead-letter queue, before
   any push.
2. **Push subscribers first.** Each now has a queue consumer for its queues
   and still serves `/_bynk/event/` for one release, so an old publisher's
   fan-out can still deliver to it during the window.
3. **Push publishers.** Each sends to queues, drops `__EventsFanout`, and
   drops its Service Bindings to subscribers it does not call.
4. **Tombstone the fan-out.** Every Worker that publishes emits
   `[exports.__EventsFanout] state = "deleted"` for a deprecation window.
   ADR 0557 records that Cloudflare treats a `deleted` tombstone for a
   namespace that does not exist as stale with no effect, so emitting it
   unconditionally is safe and needs no ledger lookup. The fan-out stored
   nothing, so no data moves.
5. **Lift the cycle refusal.** Event delivery adds no Worker-to-Worker edge,
   so `cycle_publishers` (`graph.rs:44`) no longer finds a publisher on a
   cycle. A remaining cycle is a true `consumes`-call cycle, which
   `bynk.context.consumes_cycle` already rejects earlier.

Emissions in flight on the old path at cutover are delivered or lost exactly
as today: at most once. That is the last time.

## 9. Testing

- `unit` and `integration`: `svc.event(e)` unchanged, in process.
- `system`: `svc.event(e)` is admitted and drives the subscriber Worker's
  `queue` entry with a signed one-message batch, as ADR 0522 B does for
  queues. It yields `Ack`, `Retry(reason)` or `Undecodable` (#3878 E).
- `system`, end to end: a driver that makes a publisher emit leaves the
  messages on a queue double. The harness drains them through the subscriber
  entries, applying `retries` and the dead-letter queue, so a case can observe
  a retry and a dead-letter.
- `system`, trust: a case delivers an unsigned message, one signed by another
  context, one moved from another queue, and a replay. Each is acknowledged
  and never handled.
- `workerd`: an extension of `bynkc/tests/events_ordering_workerd.rs`
  (ADR 0290) checks completeness under concurrent emission and drops the
  in-batch order assertion.

## 10. Risks

- **Outbox latency.** The agent's reply waits for the outbox write (in the
  same transaction as the state) and the hand-off. Measured in Slice 2 at the
  50th and 95th percentile against today's fan-out round trip. If the
  in-line hand-off is too slow, the agent replies first and leaves the
  hand-off to the alarm.
- **The alarm becomes shared.** An agent has one alarm. A later feature that
  wants it (scheduled agent work) must multiplex with the outbox. The runtime
  owns the alarm and schedules the earliest due time.
- **Outbox keys leak into state.** Reserved keys must be skipped by
  `loadState`, `__reportUnknownStoredKeys`, the layout check and the
  inspector. Covered by a fixture.
- **Filter skew.** A subscriber's pattern lives in the publisher's code. A
  subscriber deployed with a wider pattern than its publisher's build misses
  events until the publisher is redeployed. `bynk deploy` deploys the whole
  project, so this needs a partial deploy to happen. The handler-side guard
  catches the narrowing direction.
- **Cost.** Each matching subscriber now costs queue operations and a D1
  round trip per delivery (§14, Q9).
- **Queue sprawl.** Two queues per subscriber service per environment, against
  the account limit.
- **Service faults after side effects.** A service whose hand-off fails faults
  after its body ran. A caller's retry runs the body again. Authors who need
  commit atomicity emit from an agent; the guide says so.
- **Shared format with #3899.** If the two increments diverge, there are two
  verification paths. Settled together (§13).

## 11. Threat model

**Assets**

- Owner-only emission: only the declaring context emits `E` (ADR 0289).
- The authenticity of `EventEnvelope.publisherId` (ADR 0485).
- The subscriber's input contract: the event type, decoded and
  refinement-checked.
- Delivery itself: a subscriber must not be starved by a flood.

**Adversaries**

- User code in the same project naming an internal queue through
  `Queue @name` or `from queue`.
- Another Worker on the account with a producer binding to the queue; a
  Cloudflare product event subscription; a dashboard send; a token with
  `queues_read` and `queues_write`.
- A public client. It has no path to a queue. Once the `/_bynk/event/` route
  is removed (§8) it has none to the subscriber's event handling either.

**Where verification happens**

- At compile time: a reserved name is refused (§7.1).
- At delivery: the subscriber verifies the signature over the body, bound to
  its own queue, before decoding (§7.2). Anything that does not verify is
  acknowledged, reported and counted, never handled, never dead-lettered, so
  the dead-letter queue holds only genuine messages.
- Replays: rejected by the `eventId` claim (§7.4). A timestamp window cannot
  do this, because messages legitimately wait for days.

**Not protected**

- **Confidentiality.** Account members can read bodies through the dashboard
  or a pull token. Events must not carry secrets, as `Queue`'s guidance
  already says (`bynk-capabilities.md:755-757`).
- **Availability.** An account member can flood a subscriber queue (cheap to
  reject, but it costs invocations and delays real events) or pull and
  acknowledge messages to delete them. That depends on account access, which
  is Cloudflare's to control. `concurrency` bounds a subscriber's load.

## 12. Slices

| # | Slice | Depends on |
|---|---|---|
| 0 | Settle this doc; confirm the *to confirm* platform facts (§6); land the four ADRs (§13) | — |
| 1 | Subscriber queues: reserved names and refusal; signed messages to compiler-owned queues sent at commit; the subscriber `queue` entry (verify, dedup, decode, guard); deploy creates queues and dead-letter queues, tombstones `__EventsFanout`, drops publisher-to-subscriber bindings and the cycle refusal; `/_bynk/event/` kept for one release; `system` tier through the queue entry | 0, #3878 |
| 2 | The publisher's step: the agent outbox and alarm, with the latency measurement; the service send-or-fault rule; the size check before commit | 1 |
| 3 | Filtering at the publisher: generated predicates in the routing table; the handler guard kept | 1 |
| 4 | Subscriber delivery: `delivery { }` on `from Events` services, the compiler-owned dead-letter queue, `EventDelivery` | 1, #3877 |
| 5 | The book: the events guide's delivery sections and the spec rewritten for the new guarantee; #3897's at-most-once wording retired; `/_bynk/event/` removed | 2, 3, 4 |

Slice 1 alone is still at most once from the publisher's side (a failed send
after commit is lost), but it already removes the wait, the bottleneck and
the cycle refusal. Slice 2 is where "at least once from a committed emission"
becomes true end to end.

## 13. Decisions (ADRs to land up front)

- **D1. The delivery substrate.** One compiler-owned queue and one dead-letter
  queue per `from Events` service; publisher-side routing; no fan-out Durable
  Object; no event Service Bindings. Supersedes ADR 0284. ADR 0284 rejected
  one queue per event type because a shared queue cannot give each subscriber
  its own acknowledgement; one queue per subscriber does not have that
  problem.
- **D2. The guarantee.** At least once from a committed agent emission; for a
  service, durable hand-off or a handler fault; best effort and in process on
  `bundle`. Duplicates are absorbed by the runtime's `eventId` claim.
- **D3. Ordering.** Best effort. Amends ADR 0290 by withdrawing both partial
  guarantees.
- **D4. Trust for internal queues.** Reserved names refused at compile time;
  the signed-message format shared with #3899, bound to the destination
  queue, under the ADR 0485 edge key; verification before decoding; replay
  rejection by claim; rotation window.

Recorded in their slices: the outbox's storage layout and alarm protocol
(slice 2); the predicate generation (slice 3); `EventDelivery` and the
`delivery { }` keys admitted on events (slice 4).

## 14. Open questions — proposed answers

Proposed for review, not settled. Each is closed under review in this PR.

1. **The publisher's step.**
   *Proposed:* agents use a transactional outbox: pending messages written in
   the commit's `storage.transaction`, an alarm set as a backstop, an in-line
   hand-off after commit that deletes what it sent. Services attempt the
   hand-off after the body with a short bounded retry, then fault the handler.
   *Why:* the agent outbox is the only design that makes "committed" and
   "handed off" the same fact, and `EntryOverlay.flush` already gives one
   transaction to join. A service has no commit to join, so the honest rule is
   that its emissions are durable once it returns success, and a failure is
   visible to its caller. Slice 2 measures the outbox's added latency before
   the in-line hand-off is fixed. *Rejected:* a per-context outbox Durable
   Object for services. It brings back a hop and, without sharding, the
   bottleneck this track removes, and still cannot join the service's other
   effects into one commit.

2. **Protecting the internal queues.**
   *Proposed:* reserved `bynk-ev-` names with a digest (§7.1), refused in
   `Queue @name`, `from queue` and `deadLetter`; environment-qualified and
   recorded in the ledger as Bynk-created; every message signed with HMAC
   under the edge key, covering the destination queue, the event type (inside
   `body`), `eventId`, and the body (§7.2); verification before decoding;
   replays rejected by the `eventId` claim; previous key accepted for the
   retention window. The format is #3899's, with `body` carried as text.
   *Why:* a queue has no headers and is writable by anything on the account,
   so the proof has to travel in the body and bind to one queue. Sharing
   #3899's format gives one verifier. Text avoids canonicalising JSON.
   *Rejected:* trusting the queue's name alone, which any Worker on the
   account can bind to.

3. **Message format and limits.**
   *Proposed:* one message per (event, subscriber), the signed wrapper of
   §7.2. The publisher encodes in the handler and faults before commit if any
   message exceeds the cap. `EventEnvelope.trace` (#3895) rides in the
   envelope inside `body`.
   *Why:* a fault before commit is visible and loses nothing. Every byte of
   the wrapper is known in the handler, so the check is exact. *Rejected:*
   splitting large events across messages, or storing large payloads in R2
   and sending a reference. Both add machinery for a case that a 128 KB event
   suggests is better modelled differently.

4. **Ordering.**
   *Proposed:* best effort, with no partial guarantee. ADR 0290's two
   guarantees are withdrawn and the guide says a subscriber that needs order
   carries a sequence number.
   *Why:* queues reorder across batches and retries. Keeping order within one
   emission's batch would mean one message per batch per subscriber, which
   ties one event's retry to its siblings, complicates dedup per event and
   fills the 128 KB cap faster. A guarantee that holds only until the first
   retry is worse than none. *Rejected:* keeping within-batch order.

5. **What a subscriber's failure means.**
   *Proposed:* keep `on event … -> Effect[()]`. A fault means retry until
   `retries`, then the dead-letter queue; success means acknowledge. No
   verdict type. `delivery { }` (#3877) applies to `from Events` services
   with `retries`, `retryDelay`, `batchSize`, `batchWait` and `concurrency`;
   `deadLetter` is refused because the compiler always provisions one.
   Undecodable verified messages go to the dead-letter queue at once
   (#3878 B). `attempts` reaches the handler through an optional parameter of
   a new platform-neutral type `EventDelivery { attempts: Int }` in `bynk`,
   identified by type as #3880 C does, never through the envelope.
   *Why:* under ADR 0078's agency rule a subscriber makes no dispatch
   decision beyond success or failure; it is cron's case, not queue's. The
   envelope identifies the emission and is the same for every subscriber, so
   a per-delivery count does not belong in it. `QueueDelivery` lives in
   `bynk.cloudflare` and would tie a portable subscriber to one platform.
   *Rejected:* a `QueueResult` verdict on event handlers. Consuming the
   dead-letter queue from source is left to a follow-up proposal.

6. **Filtering at the publisher.**
   *Proposed:* the routing table carries a generated predicate per subscriber
   for its pattern and `via schema` range, evaluated on the in-memory payload
   before the outbox write. The handler-side guard stays, as a defence and for
   `svc.event(…)` in tests.
   *Why:* the patterns are literals over fields of `E` and the publisher holds
   the full value, so the check is exact and an excluded subscriber costs
   nothing. The guard catches a skewed deploy in the narrowing direction.
   *Rejected:* keeping deliver-and-filter, which pays a queue write, a read
   and an invocation for every excluded subscriber.

7. **`bundle`, Node and the browser.**
   *Proposed:* keep in-process dispatch without retry, and document the
   difference. A subscriber fault stays isolated from its siblings and from
   the publisher and is reported.
   *Why:* `bundle` is a library with no durable store, so a retry would be in
   memory, would slow the emitting call, and would still be lost on a crash.
   Matching behaviour without matching durability would mislead. *Rejected:*
   an in-memory retry loop.

8. **Migration and deploy.**
   *Proposed:* one `bynk deploy`: create queues and dead-letter queues, push
   subscribers (queue consumer plus the old route for one release), push
   publishers (queue producers, no fan-out, no event bindings), emit a
   `deleted` tombstone for `__EventsFanout` unconditionally for a deprecation
   window, mint edge keys for publisher→subscriber pairs, and lift the cycle
   refusal (§8).
   *Why:* the fan-out stored nothing, so no data moves; ADR 0557 records that
   a stale `deleted` tombstone has no effect; keeping the old route for one
   release lets an old publisher drain during the push. *Rejected:* a
   two-phase upload (ADR 0475 D3), which the removed binding makes
   unnecessary.

9. **Cost and limits.**
   *Proposed:* accept two queues per subscriber service per environment, and
   one queue write, read and acknowledgement per matching subscriber per
   emission, plus one D1 claim per delivery. `bynk deploy --plan` reports the
   queue count against the account limit and warns near it. Slice 0 writes the
   comparison with today's cost (one Durable Object request per emission plus
   one Worker invocation per subscriber) from Cloudflare's current pricing,
   which this doc does not quote (*to confirm*).
   *Why:* the cost buys durability, the end of the publisher wait, and
   publisher-side filtering, which removes today's cost for excluded
   subscribers. *Rejected:* one queue per subscriber context shared by its
   services, which saves queues but loses per-service acknowledgement and
   `delivery { }` settings, the property that motivated per-subscriber queues.

10. **Testing.**
    *Proposed:* `svc.event(…)` unchanged at `unit` and `integration`. At
    `system` it is admitted and drives the queue entry, yielding `Ack`,
    `Retry(reason)` or `Undecodable`; emissions from a driver are drained
    through a queue double that applies retries and the dead-letter queue;
    trust cases cover unsigned, mis-signed, moved and replayed messages (§9).
    *Why:* it reuses ADR 0522's queue path, so `system` exercises the real
    entry, and the outcome names match the queue tier. *Rejected:* keeping
    `bynk.test.event_needs_in_process`, which would leave the new path
    untested at the tier meant for it.
