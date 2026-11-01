---
title: Understand events
---
A context sometimes needs to tell other contexts that something happened,
without knowing — or caring — who is listening. Bynk's `event`/`Events`/`from
Events(E)` trio is in-system pub-sub: one context declares a typed fact and
emits it, any number of others subscribe to it, and the compiler — not your
code — wires the delivery.

This page is the mental model for the emit/subscribe core (slice 0),
subscription pattern filtering (slice 1), the runtime envelope (slice 2),
event field defaults (slice 3a), `@schema(N)` event versioning (slice 3b),
the cross-build schema registry that verifies it (slice 3c), and
version-aware dispatch with `via schema(N)` (slice 4) and its ranges (slice 4b).
For the track's remaining scope (replay), see
[Versioning & roadmap](/book/about/versioning-and-roadmap/).

## The three pieces

```mermaid
flowchart LR
  A["commerce.order<br/>event PaymentConfirmed"] -->|"Events.emit[E]"| B["fan-out"]
  B -->|"delivered"| C["commerce.notifications<br/>from Events(E)<br/>on event(e: E)"]
```

- An **`event`** is a typed fact, declared inside a context — a record shape,
  nothing more:

  ```bynk
  context commerce.order

  exports transparent { PaymentConfirmed }

  event PaymentConfirmed = {
    orderId: String,
  }
  ```

- **`Events`** is the first-party capability that emits one: `Events.emit[E](value)`,
  `given Events` on the handler. Like every generic capability operation
  (`Idempotency.dedup[T]`, for the same reason — see
  [First-party `bynk` capabilities](/book/reference/bynk-capabilities/)), the
  type argument is always explicit, never inferred from `value`'s type.

  ```bynk,fragment
  service markPaid {
    on call(orderId: String) -> Effect[()] given Events {
      Events.emit[PaymentConfirmed](PaymentConfirmed { orderId: orderId })
    }
  }
  ```

- **`from Events(E)`** is a service protocol — the sixth member of the closed
  protocol set alongside `call`/`http`/`cron`/`queue`/`websocket`. A service
  with this protocol has exactly one handler, `on event`, and is not called
  directly; it runs whenever a matching event arrives.

  ```bynk,fragment
  context commerce.notifications

  consumes commerce.order
  consumes bynk { Logger }

  service OnPayment from Events(PaymentConfirmed) {
    on event(e: PaymentConfirmed) -> Effect[()] given Logger {
      Logger.info(e.orderId)
    }
  }
  ```

  With no pattern on the header, every emission of `PaymentConfirmed` reaches
  every subscriber of `PaymentConfirmed`. A subscriber that wants only some
  of them can filter — see the next section.

## Filtering delivery with a pattern

A subscription may narrow which emissions it receives with a structural
pattern on the payload:

```bynk,fragment
context commerce.notifications

consumes commerce.order
consumes bynk { Logger }

service OnDomesticPayment from Events(PaymentConfirmed { region: Region.Domestic, .. }) {
  on event(e: PaymentConfirmed) -> Effect[()] given Logger {
    // e.region is NOT statically Region.Domestic here — e is
    // PaymentConfirmed as always. Only *delivery* is filtered: this
    // subscriber only runs for emissions where region == Domestic.
    Logger.info(e.orderId)
  }
}
```

A pattern lists a subset of the event's declared fields; listing any field
requires a trailing `..` (`from Events(E { })` is rejected — use the bare
`from Events(E)` form for an unfiltered subscription). A field's value is a
literal (`Int`/`String`/`Bool`) or a nullary sum-type variant, written bare
(`Domestic`) or qualified (`Region.Domestic`); a variant that carries a
payload is rejected — matching only the tag while ignoring a payload would
silently admit emissions the pattern looks like it should exclude. Multiple
listed fields compose with AND. There is no nested-field sub-pattern yet.

**Deliver-and-filter, not narrower routing.** Every emission of `E` still
reaches the fan-out mechanism; each subscriber's own generated handler
evaluates the pattern as a guard and no-ops if it doesn't match. This is why
`e`'s type is unaffected — the filter is a runtime check at the top of the
handler body, not a language-level type refinement. A future increment may
add static narrowing once the design's own open refinement-propagation
question is settled; today, a handler that needs to act on the fact that its
pattern matched still reads `e.region` at its full declared type.

## The envelope, and idempotent handling

An `on event` handler may declare a second, optional parameter —
`env: EventEnvelope` — carrying runtime metadata about the emission:

```bynk,fragment
context commerce.notifications

consumes commerce.order
consumes bynk { Idempotency }

service OnPayment from Events(PaymentConfirmed) {
  on event(e: PaymentConfirmed, env: EventEnvelope) -> Effect[()] given Idempotency {
    let seen <- Idempotency.dedup[()](env.eventId)
    match seen {
      Some(_) => (),
      None => {
        // react to e here, exactly once per eventId
        let _ <- Idempotency.remember[()](env.eventId, (), 7.days)
      }
    }
  }
}
```

`EventEnvelope` is `{ eventId: String, publisherId: String, emittedAt:
Instant, schemaVersion: Int }`. Two things worth knowing before you reach
for it:

- **`eventId` is minted once per emission, not once per delivery.** If two
  sibling subscribers both declare `env`, they observe the *same*
  `eventId` for the same emission — it identifies the emission, not the
  delivery.
- **`publisherId` names the emitting *context*, not the emitting agent.**
  `Events.emit` is legal from a plain `service` handler with no agent
  instance at all, so there is no per-agent identity to report uniformly;
  `publisherId` is the emitting context's qualified name (e.g.
  `"commerce.order"`) every time.

**The idempotency idiom.** Delivery is at-least-once (see below) — an
effectful subscriber that must not double-apply an emission dedups on
`env.eventId` using the already-documented
[`Idempotency`](/book/reference/bynk-capabilities/) capability, exactly as
shown above. This is the ordinary `dedup`/`remember` pair, not new syntax —
`env.eventId` is just a `String` key like any other, and two different
subscriber services deduping the same `eventId` never collide (each
handler's key is scoped to its own qualified name automatically). A
subscriber that only reads or transforms state, taking no effect, is
trivially idempotent already and needs no `env` parameter at all.

`schemaVersion` reflects the version the cross-build schema registry computes
for the event (below) — verified against a declared `@schema(N)` annotation
when one is present, or `1` for a brand-new event with neither.

**Testing the idiom.** A test case delivers an event straight to a
subscriber with `svc.event(e)` or `svc.event(e, env)`, at the `unit` or
`integration` tier. It runs the subscriber's pattern filter and `via schema(N)`
guard as delivery does, so delivering the same envelope twice checks the dedup:

```bynk,fragment
suite commerce.notifications {
  case "a redelivered payment is handled once" {
    let e = PaymentConfirmed { orderId: "o1" }
    let env = EventEnvelope { eventId: "evt-1", publisherId: "commerce.order", emittedAt: Instant.fromEpochMillis(0), schemaVersion: 1 }
    let _ <- OnPayment.event(e, env)
    let _ <- OnPayment.event(e, env)
    expect Idempotency.remember called once
  }
}
```

With no envelope, the case gets a fresh `eventId` on every call, the
publishing context as `publisherId`, and the event's current schema version.
A `system` case calls the handler that emits the event instead. See
[Delivering an event](/book/reference/testing/#event-address).

## Evolving an event's shape with field defaults

An event's fields can carry a default (`field: T = expr`), so a field added
later doesn't break subscribers still holding an older wire event that never
had it:

```bynk
context commerce.order

exports transparent { PaymentConfirmed, Region }

type Region = enum { Domestic, International }

event PaymentConfirmed = {
  orderId: String,
  region: Region = Region.Domestic,
}
```

If a subscriber receives a wire event whose payload has no `region` key at
all — minted before this field existed — it deserialises with the default
(`Region.Domestic`) instead of failing. This is checked at compile time: the
default must be a static, wire-representable value of the field's declared
type (a literal, a sum variant, `Some`/`None`/`Ok`/`Err`, a record, or
`T.unsafe(lit)` for an opaque type), with no reference to `self`, a
parameter, or a capability. Ordinary record types take defaults under the
same rule (see [Records](/book/reference/types/#field-defaults)); on an event,
a default is also what lets the schema registry count an added field as
additive.

**A default only rescues an absent key, not a present-but-different one.**
This matters for `Option[T]` fields in particular. For an `Option` field with a
default:

- a wire event with no key for the field uses the default;
- a key set to `null` is a real `None`, not the default;
- a key set to `{ "kind": "None" }` is a real `None` too.

In the last two cases the field was sent, just empty, which is a different fact
from "this field didn't exist yet." An `Option` field with no default decodes an
absent key as `None`. Any other field with no default still fails with a
structural-mismatch error if its key is missing, exactly as before.

## The schema registry: automatic versions, verified

Every build reconciles each event's field shape (names, types, which fields
carry a default) against `bynk.schema.lock` — a file committed alongside
`bynk.toml`, written automatically by `bynkc compile` and by `bynk dev`/`bynk
deploy`'s build step. You don't create or edit it by hand; its diff across a
pull request *is* the human-readable record of how an event's schema evolved.

For a brand-new event, the first compile baselines it at version `1` (or
at its declared `@schema(N)`, if one is present — see below). From then on,
every later compile compares the event's current shape against what the
registry last recorded:

- **Unchanged shape** — the version stays exactly what it was.
- **A purely additive change** — every added field carries a default, and
  nothing was removed or retyped — auto-bumps the version by one. No
  annotation is required for this; it happens for every event, annotated or
  not.
- **Anything else** — a field removed, retyped, added *without* a default,
  or one that lost a default it used to have — fails the build with
  `bynk.event.non_additive_schema_change`. The registry is not guessing at
  intent here: none of these are safe for a subscriber still holding an
  older wire event to decode. Give the new shape a new event type name
  instead (see [Only the declaring context may
  emit](#only-the-declaring-context-may-emit) below for why that's cheap) —
  this track's prescribed path for an actual breaking change.

```bynk,fragment
event PaymentConfirmed = {
  orderId: String,
  region: Region = Region.Domestic,
}
```

Compiling this for the first time writes:

```toml
[events."commerce.order.PaymentConfirmed"]
schema = 1
fields = [
  { name = "orderId", type = "String", default = false },
  { name = "region", type = "Region", default = true },
]
```

Add a further defaulted field later and the next compile bumps `schema` to
`2` on its own — commit the updated lock file alongside the source change,
the same way you would `Cargo.lock`.

### Checking against the lock without writing it

`bynkc check` and `bynkc test` (and `bynk check`/`bynk test`) reconcile against
the committed `bynk.schema.lock` too, but **read-only**: a non-additive change
fails them with the same `bynk.event.non_additive_schema_change` a build
reports, so a CI gate of format, check and test (such as
[`bynk-lang/bynk-ci`](https://github.com/bynk-lang/bynk-ci)) goes red on a
breaking event change before anything deploys. They never write the lock: an
additive change passes and leaves the file untouched, and only `bynkc compile`
or `bynk dev`/`bynk deploy` records the bump, so run one of those before you
commit. A project with no lock yet is a fresh project, and neither command
complains or creates one.

The editor does the same, also read-only: the language server reads the lock,
re-reads it whenever it changes (a `git checkout`, or a `bynkc compile` that
records a bump), and shows a non-additive change, a corrupt lock or a mismatched
`@schema(N)` as a diagnostic at the top of `bynk.toml`, without waiting for you
to run `bynkc check`. In VS Code, **Bynk: Show Contract Diff** lists every event
and agent class that has changed since the lock, as compatible or breaking, under
the same rules.

## Asserting a schema version with `@schema(N)`

An event may also declare its current version explicitly:

```bynk,fragment
event PaymentConfirmed @schema(2) = {
  orderId: String,
  region: Region = Region.Domestic,
}
```

`N` must be a positive `Int` literal. Unlike a plain field-shape change, this
is not silently trusted: the compiler **verifies** `N` against the version
the registry computes from the event's build history, and a mismatch fails
the build with `bynk.event.schema_version_mismatch`, naming the version the
registry actually computed. Declare `@schema(N)` when you want the version
number itself reviewable in a diff of the *source*, not only in
`bynk.schema.lock`; omit it and the registry still tracks the version for
you, silently, embedding whatever it computes into `env.schemaVersion`.

`@schema` is the only annotation an event accepts today — any other name is
rejected, and `@schema` itself may appear at most once per event.

## Dispatching by version with `via schema(N)`

A subscriber can filter delivery by the envelope's `schemaVersion`, using a
`via` clause after the `from Events(...)` header's closing `)`:

```bynk,fragment
service OnPaymentV1 from Events(PaymentConfirmed) via schema(1) {
  on event(e: PaymentConfirmed) -> Effect[()] {
    // handles the original shape
  }
}

service OnPaymentV2 from Events(PaymentConfirmed) via schema(2) {
  on event(e: PaymentConfirmed) -> Effect[()] {
    // handles the shape after the schema bumped to 2
  }
}
```

`N` must be a positive `Int` literal, matched against `env.schemaVersion` by
exact equality. A subscriber with no `via` clause receives every version,
same as before this slice. `via schema(...)` is independent of the payload
pattern from [Filtering delivery with a
pattern](#filtering-delivery-with-a-pattern) — a service may carry either,
both, or neither, in any combination.

You never need to declare `env: EventEnvelope` yourself just to use `via
schema(...)` — the compiler threads the envelope's version into the guard
whether or not your handler's own parameter list mentions it. Declare `env`
anyway if your handler body also needs another envelope field (`eventId`,
`publisherId`, `emittedAt`).

**Delivery is still deliver-and-filter, unchanged.** The fan-out mechanism
delivers every emission to every subscriber of the event type regardless of
its `via` clause; each subscriber's own generated handler evaluates the
version guard independently, exactly like the payload pattern does. This
means **sibling subscribers with the same or overlapping version coverage
are not flagged as ambiguous** — `via schema(1)` on two different services
both fire for a version-1 emission, and there is no compiler check pushing
you toward mutually-exclusive ranges. Keep sibling `via schema(...)` clauses
disjoint by convention if you want exactly-one-handler-per-version
semantics; the compiler does not enforce it for you.

## Dispatching a range of versions

`via schema(...)` also takes a range of versions, the same `lo..hi`, `lo..` and
`..hi` that [a `match` takes](/book/guides/type-system/match-on-a-range-of-numbers/),
and `_`:

```bynk,fragment
service OnV1 from Events(PaymentConfirmed) via schema(1) {
  on event(e: PaymentConfirmed) -> Effect[()] {
    // version 1 only
  }
}

service OnV2To4 from Events(PaymentConfirmed) via schema(2..4) {
  on event(e: PaymentConfirmed) -> Effect[()] {
    // versions 2, 3 and 4
  }
}

service OnV5Up from Events(PaymentConfirmed) via schema(5..) {
  on event(e: PaymentConfirmed) -> Effect[()] {
    // version 5 and later
  }
}

service OnAny from Events(PaymentConfirmed) via schema(_) {
  on event(e: PaymentConfirmed) -> Effect[()] {
    // every version
  }
}
```

- Both ends are **inclusive**, as in `InRange(lo, hi)`: `2..4` is versions 2, 3
  and 4. `2..` is version 2 or later and `..4` is version 4 or earlier.
- Every bound must be a positive `Int` literal
  (`bynk.event.bad_schema_dispatch`), and `lo` must not exceed `hi`
  (`bynk.types.empty_range_pattern`, whose fix swaps the bounds).
- `_` matches every version, exactly like omitting the clause. It is for writing
  every case of a family of subscribers explicitly; the envelope is still
  threaded to the handler.
- As with a literal, sibling subscribers whose ranges overlap, or leave a gap, are
  **not** flagged: delivery is still deliver-and-filter.

## Only the declaring context may emit

`Events.emit[E]` compiles only when `E` is an event declared **in the emitting
context itself** — even though `E` is visible cross-context for subscription
via the ordinary `consumes` you'd expect (`commerce.notifications` above
`consumes commerce.order` precisely to name `PaymentConfirmed` in its `from
Events(...)` header). A foreign context attempting to emit it fails closed:

```
context commerce.notifications
consumes commerce.order
consumes bynk { Events }

service leak {
  on call() -> Effect[()] given Events {
    Events.emit[PaymentConfirmed](PaymentConfirmed { orderId: "x" })
  }
}
→ [bynk.event.emit_outside_owner] `PaymentConfirmed` is not declared in this
  context — only the context that declares an event may emit it
```

This is deliberate, and new: every other cross-context boundary in Bynk
governs what a context may *name* (`uses`/`consumes`) — this is the first that
restricts what a context may *do* with something it can already see. A type
being subscribable does not make it forgeable.

## Emission is fire-and-forget, and release-at-commit

`Events.emit[E]` returns `Effect[()]` — the emitting handler never learns
whether a subscriber ran, or how it went. What it *does* guarantee: an
emission only ever reaches a subscriber if the emitting handler itself
**committed**. An agent handler that emits and then goes on to violate one of
its own invariants emits nothing — the invariant violation throws before the
event is released, exactly as if `Events.emit` had never been called:

```bynk,fragment
agent Ledger {
  key id: String
  store total: Cell[Int] = 0

  invariant total_stays_small:
    total < 10

  on call bump(amount: Int) -> Effect[()] given Events {
    let _ <- total.update((n) => n + amount)
    do Events.emit[PaymentConfirmed](PaymentConfirmed { orderId: "ledger-event" })
  }
}
```

Calling `bump` with an amount that pushes `total` past `10` throws
`InvariantViolation` — the state write never commits, and the emission above
it never reaches `OnPayment` either. This is not a special case for `Events`:
it is the same all-or-nothing handler-commit boundary agents already have
(store writes vs. invariants), extended to cover an emission raised anywhere
in the same handler body, including one raised inside a plain `service` that
calls into the agent.

## Delivery, concretely, per platform

The fan-out substrate differs by target, but the emit/subscribe surface above
does not — the same source compiles and runs the same way on every platform.
A subscription pattern's guard runs as the first line of the generated
handler on every target too — deliver-and-filter, not a routing difference:

| Target | Mechanism |
|---|---|
| **Cloudflare Workers** | Each publishing context gets its own compiler-synthesised fan-out Durable Object. One subscriber's delivery failure is caught and logged without blocking delivery to its siblings. Ordering is preserved *within* one emission (everything a single handler invocation emits) and *across* successive, non-overlapping calls to one agent — but **not** across concurrent invocations of the same agent, since the Durable Object delivers by making an outbound call per subscriber and does not serialise two overlapping deliveries against each other. |
| **Bundle** (node, browser) | Dispatch is in-process — no Durable Object, no wire. The composed program calls directly into each subscriber's handler. |

On Workers, delivery travels over a Service Binding from the publisher to each
subscriber. A subscriber that `consumes` its publisher only to name the event
calls nothing, so it has no binding back, and `bynk deploy` uploads the
subscriber first and the publisher second. A subscriber that also **calls** a
service of its publisher, directly or through other contexts, makes the two
Workers bind each other. Cloudflare rejects an upload whose binding target
doesn't exist yet, so neither Worker can go first, and `bynk deploy` refuses
that project and names the publisher. `bynk dev` serves it fine. To deploy it,
move the services the subscriber calls out of the publishing context.

What the track does **not** yet give you: a delivery retry, a durable log to
replay from, or ordering across *concurrent* invocations of the same
publishing agent — measured, not assumed, and found not to hold. A subscriber
that needs a total order across concurrent publishes must carry its own
sequence number in the event payload. A subscriber that must not double-apply
a redelivered emission dedups on `env.eventId` via `Idempotency` (see "The
envelope, and idempotent handling", above) — though only up to that
capability's current, in-memory limit, since a duplicate delivered across an
isolate restart is not deduped until a durable provider exists.

**See also:**
[First-party `bynk` capabilities](/book/reference/bynk-capabilities/),
[Understand the capability model](/book/guides/effects-and-capabilities/understand-the-capability-model/),
[Understand: invariants as contracts](/book/guides/agents-and-state/understand-invariants/),
[Diagnostics](/book/reference/diagnostics/).
