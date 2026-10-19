# 0294 — The `env.eventId`-keyed dedup idiom ships as documented convention, not new capability syntax, and corrects a design-notes API mismatch

- **Status:** Accepted (v0.240)

**Context.** §12 (lines 656–666) shows `<- Idempotency.dedup(on: e.eventId,
expiresAfter: 7d)?` — labelled arguments, a `?`-postfix short-circuit, and
`eventId` read directly off the event *payload* rather than an envelope.
None of this matches the shipped capability
(`bynk-check/src/firstparty/bynk.bynk`):

```
capability Idempotency {
    fn dedup[T](key: String) -> Effect[Option[T]]
    fn remember[T](key: String, value: T, expiresAfter: Duration) -> Effect[()]
}
```

Two positional operations, no `on:`/`expiresAfter:` labels on `dedup`, no
`?`-shortcut sugar, `key: String` only (settled narrower than a generic key
type in the retired idempotency track §3.4). This is the same class of gap
`design/tracks/events.md` §3.3 found in §7's pattern-filtering example
before slice 1, and #939's own DECISION B found in §7's original
`Events.emit` call shape — a design-notes worked example describing an API
that was never actually built.

**Decision.** No new capability syntax is added for this idiom. The already
-shipped `match`-based `dedup`/`remember` pair (proven end to end for the
first time combined with Events by
`bynkc/tests/fixtures/positive/961_events_envelope_idempotency_dedup`) is
the idiom, keyed on `env.eventId` — a plain `String`, so it type-checks
directly against `dedup[T](key: String)` with no coercion. This also
settles `design/tracks/events.md` §4's own open question ("settle here
whether it is bespoke syntax or documented convention") in favour of
convention: `scope_idempotency_key` (`bynk-emit/src/emitter/lower.rs`)
already prefixes every `dedup`/`remember` call's key with the calling
handler's own qualified name, so two different subscriber services deduping
the same emission's `eventId` can never collide — inherited unchanged from
#934, nothing new needed to make the idiom safe.

**A narrower, pre-existing checker limitation found while proving the
"envelope with no capability" case (named here, not fixed — out of
scope).** `bynkc/tests/fixtures/positive/962_events_envelope_bare_consumes_bynk`
set out to prove a subscriber can access `EventEnvelope` via a bare
`consumes bynk` (no braces, no capability at all) — confirmed true for
*naming* the type in a parameter position (`env: EventEnvelope` resolves
and type-checks). Plain field access on it via a bare `let` binding
(`let _ = env.eventId`), however, fails with `bynk.types.method_not_found`
— reproduced identically on an ordinary cross-context record field
(`e.orderId`, no adapter involved), so this is a general, pre-existing gap
in bidirectional field-access resolution unrelated to Events or
`EventEnvelope` specifically, simply never previously exercised because
every shipped fixture only ever accesses a cross-unit record field from an
*expected-type* position (a capability call argument, e.g.
`Idempotency.dedup[T](env.eventId)`, which does resolve correctly) rather
than a bare synthesised `let`. Out of scope for this track to fix; named as
a candidate for its own future increment.
