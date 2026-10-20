---
title: Test tiers
---
A Bynk test declares *how much of the real world is present when it runs* with an
**`as <tier>` clause** on its `case` (or `suite`) header. The three tiers are the
testing pyramid — `unit`, `integration`, `system` — and they are **not three kinds
of test**. There is **one body**; a tier only changes how much of it is real.

| Tier | What is real | Wire crossed? |
|------|--------------|---------------|
| `unit` (default) | the unit under test, in process; collaborators may be stubbed with `stub` | no |
| `integration` | real collaborators **within one context** | no |
| `system` | contexts stood up as the Workers they deploy as | **yes** — the real serialise → JSON → deserialise edge |

`unit` is the default and is **never written**. You reach for `integration` by
adding one word to the header, and the body does not change. `system` is a
suite of its own (see below).

## One body: `unit` and `integration`

Between `unit` and `integration`, promotion changes only the header. The body is
byte-for-byte identical:

```bynk,ignore
case "a small order prices with tax"                { … }  // as unit (default)
case "a small order prices with tax" as integration { … }  // real collaborators, one context
```

At `unit` a collaborator's provision is under your control (you may stub a seam
with [`stub`](/book/reference/testing/#stub)). At `integration` the real
collaborators *within the context* run, so their own invariants and contracts run
too. A green `unit` case that *fails* when promoted means a collaborator's
invariant has caught a defect a stub was hiding, with **no new test code**.

Neither tier crosses into **another context**. A consumed context isn't stood up
in-process, and `stub` doubles capabilities, not a context's services, so a
`unit` or `integration` case, or a `property`, that reaches another context's
service (directly, or through a service or agent handler that calls one) is
[`bynk.tier.cross_context_needs_system`](/book/troubleshooting/integration-errors/#bynktiercross_context_needs_system).
That flow belongs in a `system` suite.

## `system`: a suite of its own

A `system` case runs against the contexts stood up as the Workers they deploy
as, so it addresses a service by its **context path** rather than
`service.call(…)`:

```bynk,ignore
suite shop.orders as system {
  case "a small order authorises end to end" {
    let r <- shop.orders.place(100)   // deployed Workers, real wire
    expect r is Ok(_)
  }
}
```

A `system` suite holds `system` cases only. A suite that mixes them with `unit`
or `integration` cases is
[`bynk.tier.mixed_system_suite`](/book/troubleshooting/integration-errors/#bynktiermixed_system_suite):
keep each target's in-process cases and its `system` cases in separate suites.

### Configuring the Workers: a fake provider

A `stub` has no seam to override at `system`, because each Worker builds its own
providers from its `env`. To run a flow with a fake provider, give the suite the
variables that choose it with an [`env`](/book/reference/testing/#env) clause.
The entries go on every participant's `env`, and a case's `env` overrides a name
for that case only:

```bynk,ignore
suite shop.orders as system {
  env {
    PAYMENTS_PROCESSOR = "fake"
    AUTH_JWT_SECRET = "test-secret"
  }

  case "a declined payment releases the stock it reserved" { … }

  case "without a processor key the order is a 503" {
    env { PAYMENTS_PROCESSOR = "live" }
    …
  }
}
```

The same `env` works below `system`, where it is what the `Secrets` double
answers, so a case body keeps its configuration when it moves between tiers. An
entry naming a `Bearer` actor's secret (`AUTH_JWT_SECRET` here) is the secret the
driver signs tokens with and the Worker's seam verifies them with.

## `as` on the `suite` header

`as` also sits on the `suite` header, setting a default every `case` inherits and
may override between `unit` and `integration` (the case wins):

```bynk,ignore
suite checkout as integration {          // every case defaults to integration…
  case "small order authorises"       { … }  // as integration (inherited)
  case "a unit-level edge"    as unit { … }  // case overrides the suite default
}
```

A case's effective tier is `case.tier ?? suite.tier ?? unit`. `system` doesn't
mix: a `suite … as system` can't hold a case that steps down, and a case can't
step up to `system` beside lower-tier ones.

Tiers are a **`case`-only** affordance. A `property` *generates* and does not
promote, so a suite-level `as` binds its `case` members only; an `as` on a
`property` header [does not parse](/book/troubleshooting/integration-errors/#a-tier-on-a-property).
To check a generated input end to end, promote *that witness* as a concrete
`case … as integration`.

## Participants are inferred, not listed

For `system` the compiler already knows which contexts to stand up: it walks the
unit under test's transitive **`consumes` graph**. There is **no
participant list to maintain** — an explicit list could only drift from the real
dependency graph. (The old `suite integration "…" { wires … }` form and its
`wires` clause are retired.)

A `system` case whose inferred set is fewer than two contexts — the target and at
least one consumed context — is
[`bynk.tier.system_needs_wire`](/book/troubleshooting/integration-errors/#bynktiersystem_needs_wire):
`system` describes the *cross-context, wired* tier, so it needs a wire to cross.
To exercise one context's real collaborators without a wire, use `as integration`.

## When to promote

- **Stay at `unit`** for the common case — a scenario about one unit's logic, with
  collaborators you control seam by seam.
- **Promote to `integration`** when the point is a unit *with its real
  collaborators in one process* — no stub standing in for behaviour you actually
  want to exercise. This is the missing middle: real collaborators, one context,
  no wire.
- **Promote to `system`** when the point is a flow **across contexts** — the real
  serialise → JSON → deserialise → projection path that only the Workers target
  emits, which no in-process tier touches.

The pyramid's proportion advice rides along for free: **many `unit`, some
`integration`, few `system`**.

## A worked example

Two contexts: `shop.payment` authorises a charge; `shop.orders` consumes it.

```bynk
context shop.payment

exports transparent { PayError }

type PayError = enum { Declined }

capability Bank {
  fn charge(cents: Int) -> Effect[Result[Int, PayError]]
}

provides Bank = StubBank {
  fn charge(cents: Int) -> Effect[Result[Int, PayError]] {
    if cents > 10000 { Err(Declined) } else { Ok(cents) }
  }
}

service authorise {
  on call(cents: Int) -> Effect[Result[Int, PayError]] given Bank {
    let r <- Bank.charge(cents)
    r
  }
}
```

```bynk,fragment
context shop.orders

consumes shop.payment as Pay

exports transparent { OrderError }

type OrderError = enum { Rejected }

service place {
  on call(cents: Int) -> Effect[Result[Int, OrderError]] {
    let a <- Pay.authorise(cents)
    match a {
      Ok(n)  => Ok(n)
      Err(_) => Err(Rejected)
    }
  }
}
```

A `system` suite exercises the flow end to end. Its participants — `shop.orders`
and `shop.payment` — are **inferred** from `shop.orders`'s `consumes`, so there is
nothing to wire by hand:

```bynk,fragment
suite checkout as system {
  case "small order authorises across the wire" {
    let r <- shop.orders.place(100)
    expect r is Ok(_)
  }

  case "large order is rejected end to end" {
    let r <- shop.orders.place(50000)
    expect r is Err(_)
  }
}
```

`shop.orders.place(100)` enters the orders Worker; inside, `place` calls
`shop.payment.authorise(100)`, which crosses a simulated Service Binding into the
payment Worker — serialising the argument and deserialising the result for real.
Both hops are exercised.

## The state lifecycle is fixed across tiers

Promotion is honest because the **unit under test is always a real in-memory
instance, keyed normally, fresh per case** — at every tier. Only the realness of
its *collaborators* and whether sends cross a serialisation boundary change with
the tier. Snapshot and step invariants are checked at the commit boundary, so they
fire at **every** tier for free — which is exactly why promoting a `unit` case can
surface a collaborator invariant a stub was hiding, with no new test code.

**Agents** (Durable Objects) work across the `system` wire: a participant's agents
are backed by in-memory instances — same key, same instance **within a case**,
with state starting empty and resetting **per case**. So a service that drives an
agent can be exercised end to end, and you can assert on accumulated state.

## Run it

```sh
bynkc test .
```

```text
Running tests...

checkout:
  ✓ small order authorises across the wire
  ✓ large order is rejected end to end

2 passed, 0 failed.
```

For a `system` suite, `bynkc test` compiles the inferred participants in workers
mode under `out/workers/`, stands each one up as an in-process Worker, wires the
bindings, type-checks everything with `tsc --strict`, and runs it on Node. No
`wrangler` or `miniflare` is needed.

See the [testing reference](/book/reference/testing/#tiers-the-as-tier-clause), the
[`stub`](/book/reference/testing/#stub) seam override, and
[`bynk.tier.*` / `bynk.stub.*` errors](/book/troubleshooting/integration-errors/).
