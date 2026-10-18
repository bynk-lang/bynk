---
title: "`bynk.tier.*` and `bynk.stub.*` errors"
---
These diagnostics come from the **tier dial** (the `as <tier>` clause) and from
**`stub`** test doubles (v0.118). See [Test tiers](/book/guides/testing/integration/),
the [`stub` reference](/book/reference/testing/#stub), and the
[tiers reference](/book/reference/testing/#tiers-the-as-tier-clause).

## A tier on a `property`

```text
[bynk.parse.expected_token] expected `{` to open the property body, found `as`
```

**Cause:** an `as <tier>` clause is attached to a `property` header. Tiers are a
`case`-only clause, so the grammar has no place for one on a `property`. A
`property` *generates* its subjects and does not promote — promoting it would
multiply generation by real-collaborator cost and re-admit the ambient
nondeterminism a tier removes.

**Fix:** remove the tier from the `property`. A suite-level `as` binds its `case`
members only, so a `property` under a tiered suite is fine. To check a generated
input end to end, promote *that witness* as a concrete `case … as integration`.

## `bynk.tier.system_needs_wire`

```text
[bynk.tier.system_needs_wire] `system`-tier suite for `shop.orders` has no serialisation edge — the target consumes no other context and exposes no `http` or `queue` service
```

**Cause:** a `system` suite has nothing to serialise across. Its target consumes
no other context, and it exposes neither an `http` service (its public boundary)
nor a `queue` service (whose messages the platform serialises). A `cron`-only
target is the common case: a scheduled event carries only a cron string and a
time.

**Fix:** if the flow genuinely stays within one context, use `as integration` (real
collaborators, one context, no wire) instead. If it should cross a boundary, make
sure the unit under test actually `consumes` the other context — participants are
inferred from that graph, never listed.

## `bynk.tier.cross_context_needs_system`

```text
[bynk.tier.cross_context_needs_system] case `"pays"` calls `check`, which calls `shop.payment.authorise` in another context, but it is a `unit`-tier case
```

**Cause:** a `unit` or `integration` case, or a `property`, reaches another
context's service: directly (`Payment.authorise(…)`), or through a service or
agent handler of the unit under test that calls one. A property has no tier and
always runs in-process, so it can't cross a context at all; drive that flow
from a case in a `system` suite. Below `system` a case runs in-process. No
consumed context is stood up, and `stub` doubles *capabilities*, not a context's
services, so there would be nothing on the other side of the call.

**Fix:** move the case into a `system` suite, which stands the contexts up as
Workers on either side of the real wire. There a service is addressed by its
context path:

```bynk,ignore
suite shop.orders as system {
  case "pays" {
    let r <- shop.orders.check(100)
    expect r is Ok(_)
  }
}
```

To test the unit's own logic in-process instead, drive a service that stays in
the context.

## `bynk.tier.mixed_system_suite`

```text
[bynk.tier.mixed_system_suite] case `"pays"` is `as system`, but other cases in its suite run below `system`
```

**Cause:** a suite mixes `system` cases with `unit` or `integration` ones. A
`system` case runs against deployed Workers and addresses services by context
path (`shop.orders.place(…)`), while a lower-tier case calls `place.call(…)`
in-process, and one suite is emitted one way or the other, not both.

**Fix:** keep a target's in-process cases and its `system` cases in separate
suites, one `suite shop.orders { … }` and one `suite shop.orders as system { … }`
(they may sit in separate files).

## `bynk.stub.not_a_seam`

```text
[bynk.stub.not_a_seam] `Rates` is not a capability the unit under test consumes; only a consumed capability can be provided
```

**Cause:** a `stub` clause targets something that is not a capability seam the
unit under test consumes / has in scope via `given` — for example an agent, a type,
or a capability the unit does not depend on. `stub` is **capability-only**: an
agent's realness is the tier's job, not a provider's.

**Fix:** provide a capability the unit actually consumes. To change the realness of
an agent or a whole context, promote the tier (`as integration` / `as system`)
instead.

## `bynk.stub.at_system`

```text
[bynk.stub.at_system] `stub Secrets.get` is in a `system`-tier case, where a `stub` has no effect
```

**Cause:** a `stub` clause sits in a `system`-tier case, or at the scope of a
suite that runs `system` cases (`suite … as system`, or one whose cases are
`as system`). A `system` case runs each context as the Worker it deploys
as, and that Worker builds its own capability providers, so the stub has no seam
to override.

**Fix:** move the case into a suite below `system` (`unit`, or `as
integration`), where a `stub` overrides the seam. Keep the `system` case for what
only the wire shows: the serialised request and response.

## `bynk.stub.unknown_op`

```text
[bynk.stub.unknown_op] capability `Rates` has no operation named `looup`
```

**Cause:** the `Cap.method(…)` left-hand side names an operation the capability does
not declare (typically a typo).

**Fix:** use one of the capability's declared operations (check the `capability`
block).

## `bynk.stub.rhs_type`

```text
[bynk.stub.rhs_type] `returns "1.25"` has type `String`, but `Rates.lookup` returns `Float`
```

**Cause:** a `returns <value>` supplies a value whose type disagrees with the
operation's declared return type.

**Fix:** return a value of the operation's result type. To inject a *fault* rather
than a value, write `fails`; an in-band `Err` outcome is an ordinary value you
assert directly in the case.

## `bynk.stub.bad_sequence`

```text
[bynk.stub.bad_sequence] `returns each []` is empty; a sequence needs at least one outcome
```

**Cause:** a `returns each [<outcome>, …]` sequence is malformed — most commonly
empty, so there is no outcome to serve on the first call.

**Fix:** give the sequence at least one outcome. Each outcome is a value (a
success), the atom `fails` (a fault), or `ok(v)`; the **last outcome repeats** once
the sequence is exhausted, so `[fails, fails, ok(resp)]` is "fails twice, then
succeeds forever".
