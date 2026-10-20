---
title: Share a capability across contexts
---
**Goal:** write a capability once — a `Clock`, an `Http` client, a `Random`
source — in a **platform context**, and let application contexts depend on it
without re-declaring or re-implementing it.

## Export the capability from the providing context

The providing context declares and provides the capability as usual, then lists
it in an `exports capability { … }` clause:

```bynk
context platform.time

exports capability { Clock }

capability Clock {
  fn now() -> Effect[Int]
}

provides Clock = SystemClock {
  fn now() -> Effect[Int] {
    0
  }
}
```

Only a capability the context both **declares** and **provides** may be exported
(otherwise a consumer could not instantiate it).

## Consume it with a qualified `given`

The consumer `consumes` the providing context and depends on the capability
through a **qualified `given`** — the same prefix is used for the call:

```bynk,fragment
context ops.jobs

consumes platform.time

service tick {
  on call() -> Effect[Int] given platform.time.Clock {
    let t <- platform.time.Clock.now()
    t
  }
}
```

Prefer an alias for brevity when the context path is long:

```bynk,ignore
consumes platform.time as Time
// ...
on call() -> Effect[Int] given Time.Clock {
  let t <- Time.Clock.now()
  t
}
```

A consumer's *provider* may depend on a cross-context capability too:
`provides Stamp = ClockStamp given platform.time.Clock { … }`.

## How it's wired

The capability's **contract** is imported for type-checking; its **provider** is
instantiated in the consumer's own composition — in the shared root (bundle) or
imported into the consuming Worker (workers). The call runs **in-process**
(`deps.Clock.now()`), so each consuming Worker gets its own instance — exactly
what stateless platform capabilities want. No Worker hop is involved.

## The rules

- `exports capability` names must be declared **and** provided
  ([`bynk.exports.undeclared_capability`](/book/troubleshooting/exports-capability-errors/),
  [`bynk.exports.capability_not_provided`](/book/troubleshooting/exports-capability-errors/)).
- `given B.Cap` requires `B` to be `consumes`-d (otherwise
  `bynk.resolve.unconsumed_context`) and to export `Cap`
  ([`bynk.given.cross_context_unknown_capability`](/book/troubleshooting/exports-capability-errors/)).

## Related

- Reference: [Capabilities & providers](/book/reference/capabilities/).
- [Compose a provider from other capabilities](/book/guides/effects-and-capabilities/compose-a-provider/).
- [Consume another context's services](/book/guides/program-structure/consume-services/).
