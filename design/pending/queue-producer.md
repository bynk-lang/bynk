---
level: minor
probes: emit_diagnostics, emit_abi_shapes, ts_writes
changelog: "`bynk.cloudflare` gains `Queue`, a Cloudflare Queue producer as an instance-only derived capability (`capability OrderEvents = Queue @name(\"order-events\")`) with `send`, `sendWith` (a delay) and `sendBatch` over JSON text from `Json.encode`; `bynk deploy` creates a missing producer queue, recording it only when it created it (standard-library P2, [#2578](https://github.com/bynk-lang/bynk/issues/2578))"
---

## ADR: queue-producer-derived-capability
title: Queue producers as instance-only derived capabilities
summary: Cloudflare's queue producer binding as an instance-only `Queue` named by `@name` (not ADR 0469's `@queue`, a keyword), JSON-text bodies sent parsed so a Bynk consumer decodes them, and producer-only queues recorded in the ledger only when Bynk created them

**Context.** A Bynk service could consume a queue (`service … from queue("n")`,
ADR 0078) but not write to one. ADR 0002 left the producer side waiting for a
capability home, and ADR 0122 D2 named it: the platform `Queue` capability. The
standard-library track settled its shape: an instance-only derived capability
(ADR 0469), whose queue name the source picks (track A8, A12), with bodies the
caller encodes with `Json.encode` (track D4). Cloudflare's producer binding is
`env.X.send(body, { contentType, delaySeconds })` and `env.X.sendBatch(messages)`,
configured by a `[[queues.producers]]` stanza (`binding`, `queue`). Three facts
from the code shaped the decision. `queue` is a lexer keyword, so `@queue("…")`
does not parse. The generated consumer decodes `msg.body` as an already-parsed
JSON value. And `bynk deploy` records a consumer's queue even when it only
reused one that was already there.

**Decision.**

- **A. `Queue` is an instance-only derived capability with a required `@name`.**
  An `Instantiable` row (prefix `QUEUE`, `bare: false`, option `name` a string):
  `capability OrderEvents = Queue @name("order-events")` binds as
  `QUEUE_ORDER_EVENTS`. This **replaces ADR 0469 D3's `@queue("…")`** as the
  queue producer's option. `queue` is a keyword, and re-admitting it as an
  annotation name is grammar work (ADR 0023), as `Service`'s `@worker` already
  found. The name must be 1 to 63 lowercase letters, digits or `-`, starting
  and ending with a letter or digit. Cloudflare allows letters, digits and
  dashes; lowercase only is stricter, matches every name `bynk deploy` derives,
  and can be relaxed later without breaking anyone. Anything else is
  `bynk.capability.invalid_option`, a missing name
  `bynk.capability.missing_option`, and a bare use
  `bynk.capability.instance_only`. The consumer's own name check (empty only)
  is unchanged. Tightening it would break existing consumers, so it is a
  language change of its own.
- **B. Three operations over JSON text.** `send(body: String)`,
  `sendWith(body: String, options: QueueSend)` (with `QueueSend = { delay:
  Option[Duration] }`) and `sendBatch(bodies: List[String])`, each
  `Effect[()]`.
  - **Encoding.** The caller encodes with `Json.encode`, as D4 settled. A typed
    `send[T]` (the `Events.emit[E]` precedent) stays a later upgrade, and with
    it any check that a producer's message matches a consumer's.
  - **Sending.** The binding parses each body and sends the value with
    `contentType: "json"`, so `msg.body` arrives as the parsed value the
    generated consumer already expects. A Bynk producer and a Bynk consumer
    interoperate with no consumer change, and a non-Bynk consumer receives
    plain JSON. The parse is lossless for anything `Json.encode` writes: an
    `Int` is a JS safe integer (#1657), and there is no decimal type.
  - **Faults.** Text that is not JSON faults the send, and so does a delay
    below 0 or over 12 hours. A delay is whole seconds, with a part second
    rounding up, so a message is never early. `sendBatch` parses every body
    before it sends any of them. Over-cap messages and batches fault with the
    platform's error, as `Kv.put` does: no `QueueError` yet.
  - **Rejected.** Forwarding the text with `contentType: "text"` would need the
    consumer to parse a string, a change to every consumer's decoding.
- **C. The emitted name is the source's; `bynk deploy` qualifies it.** The
  stanza carries `queue = "order-events"`, and the `[env.<name>]` block
  qualifies producers exactly as it qualifies consumers (`order-events-staging`).
  So a producer and a consumer that name one queue meet in every environment,
  and staging never sends into the default environment's queue.
- **D. Deploy creates a missing producer queue, but records it only if it
  created it.**
  - **Creation.** `read_resources` reads `queues.producers`, and provisioning
    checks and creates the queue the same way it does a consumer's, once per
    run.
  - **Recording.** A queue the project consumes is recorded either way, as
    before; a consumed queue another context already checked that run is now
    recorded too, so deploy order changes nothing. A queue that only producers
    name is recorded only when this run created it.
  - **Why.** Such a queue may belong to a Worker outside the project. A recorded
    queue becomes an orphan `--prune` can delete once nothing names it, which
    is exactly track §12's provisioning hazard.
  - **Orphans and CI.** A queue a context sends to keeps a recorded queue live
    in `find_orphans`. The plan lists producer queues beside consumed ones.
    Unrecorded queues still aren't gated in CI, because the name comes from
    source.
  - **Remaining risk.** The consumer path's reuse-then-record behaviour, and so
    its version of the same risk, is unchanged here.
- **E. Tests at every tier; experimental.**
  - **Doubles.** Each instance's `unit` double accepts every send, checking
    bodies and delay as the binding does. At `integration` and `system` each
    participant's `env.QUEUE_<NAME>` is a fresh recorder
    (`makeIntegrationQueue`). Nothing is delivered to a consumer participant.
  - **Binding tests.** The binding has R-13 tests and a `workerd` smoke.
  - **Experimental.** `Queue` is behind `[unstable] allow =
    ["bynk.cloudflare.Queue"]` until a first-party service has sent on it.
  - **The D4 ADR waits.** D4's front-loaded two-step-payload ADR (track §10 #5)
    waits for P4, the first slice that *returns* a structured payload. This
    one only encodes.

**Consequences.** A context sends to a queue in one declaration and one call,
and a Bynk consumer of the same queue decodes it unchanged. A body that does
not match the consumer's message type is caught only at the consumer, which
retries it. A queue a producer adopts from outside the project is never pruned,
and a queue Bynk created for a producer is. Completion offers `Queue` after
`capability X =` and the `@name("order-events")` snippet after it, and a
producer's hover shows its emitted stanza. Existing goldens change only where
they embed the `bynk.cloudflare` binding and adapter.
