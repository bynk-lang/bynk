---
level: minor
probes: PLACEHOLDER
changelog: Ahead of durable event delivery, each `from Events` service's Worker consumes its compiler-owned event queue (a `[[queues.consumers]]` with a dead-letter queue, `D1_IDEMPOTENCY` for its `eventId` claims), verifying each message as signed by the event's owner before decoding it, and a `system` case's `svc.event(e)` now delivers through that queue instead of being refused (`bynk.test.event_needs_in_process` is retired); nothing sends to the queues yet (durable event delivery track, slice 3b)
---
