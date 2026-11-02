---
level: minor
probes: emit_diagnostics, ts_writes, diagnostic_coverage
changelog: "`bynk.cloudflare` `Queue` gains `sendValueWith(message, options)` and `sendValueBatch(messages)`, the `Codable` counterparts of `sendWith` and `sendBatch`: the same messages, delay and all-or-none rule as the untyped calls with `Json.encode`, encoded at the call site. `Analytics.write[T]` is not added (#2793)"
---

## ADR: queue-typed-send-variants
title: Queue gains typed delayed and batch sends; Analytics.write is not made generic
summary: sendValueWith and sendValueBatch mirror the untyped set; a generic Analytics.write has no Codable meaning

**Context.** ADR 0696 shipped only `Queue.sendValue[T: Codable]`. A program that
needed a delay or a batch went back to `sendWith(Json.encode(v), …)` and
`sendBatch(xs.map(Json.encode))`, the hand-encoding slice 3 removed for the plain
send. Proposed in #2793.

**Decision.**
1. **Variants (A1).** `sendValueWith[T: Codable](message: T, options: QueueSend)`
   and `sendValueBatch[T: Codable](messages: List[T])`, one typed variant for each
   untyped one that has a typed need. No `sendValueBatchWith`: the untyped set has no
   `sendBatchWith`, and a delay on a batch starts from the untyped side.
2. **Names (B1).** They extend `sendValue` and sort beside it.
3. **`Analytics.write[T]` is not added (C1).** A `Codable` record has no meaning as
   a data point: the binding would have to invent which field is the index, which
   are the `blobs` and which the `doubles`, invisible in the signature and a new
   fixed mapping. `DataPoint` is already the typed record.
4. **Batch semantics (D).** The call site encodes the whole `List[T]` before anything
   is sent, so an element that fails to serialise faults the call with nothing sent,
   as `sendBatch` parses every body first. The platform caps (100 messages, 256 KB)
   are left to the platform, as for `sendBatch`.
5. **Delay (E).** The binding's `delaySeconds` is reused unchanged: whole seconds,
   a part second rounds up, a delay outside 0 to 12 hours faults.

**Consequences.** No checker or emitter rule changes: slice 3 already encodes a
`Codable` parameter, `List[T]` included, and infers `T`. The binding, the unit-tier
double, the first-party doc table and every vendored `bynk.cloudflare` output move
because `Queue` gains two operations. Binding and workerd tests compare each variant
with its untyped twin.
