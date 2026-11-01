---
title: Ask two sources and take the first answer
---
**Goal:** answer as fast as the quicker of two places that can both answer, without
waiting for the slower one.

## Write `race`

Add `.race(other)` to an effect. Both effects are already running when `race` is
called, because an effect starts as soon as it is built, and the result is the first of
them to **succeed**:

```bynk
context shop.prices

consumes bynk

capability Prices {
  fn get(region: String, sku: Int) -> Effect[Int]
}

service prices {
  on call(sku: Int) -> Effect[Int] given Prices {
    Prices.get("east", sku).race(Prices.get("west", sku))
  }
}
```

Both sides have the same type, and so does the result. To race effects of different
types, map them into one type first. `race` chains: `a.race(b).race(c)` is the first of
the three to succeed, and it composes with [`within`](/book/guides/effects-and-capabilities/bound-how-long-a-call-may-take/):

```bynk,fragment
let r <- Prices.get("east", sku).race(Prices.get("west", sku)).within(2.seconds)
```

## What wins

- **The first success.** A side that faults quickly does not beat a side that answers
  later: the race waits for the other.
- **If every side faults,** the first fault in source order is raised, with the others
  attached to it as `suppressed`. The fault does not depend on which was quickest.
- **A tie goes to the receiver.** When two sides finish at the same instant, the one
  written first wins.
- A `Result` is a value, so an `Err` that an effect returns is a success and can win.
  If you want the first `Ok`, make an `Err` a fault before you race.

## Race only work that is safe to do twice

The loser is **not cancelled**. It is handed to the platform's `waitUntil` and runs to
completion, so its side effects still happen. Use `race` for reads, and for writes that
are idempotent. Racing two payments pays twice. It also does the work twice, so it
costs twice what one call does: it pays when the latency you save is worth that.

Use `FetchOptions.timeout` when work must actually stop. For more, read
[what a timeout stops](/book/guides/effects-and-capabilities/what-a-timeout-stops/).

## Where it is allowed

The same places as `within`: a **service handler's body**, lambdas included, and event,
cron and queue handlers. It is refused in an agent handler
(`bynk.race.in_agent_handler`), and in a function, a provider operation or a WebSocket
handler (`bynk.race.outside_handler`): none has an execution context to hand the loser to.

## Test the race

A test runs the race on virtual time, so it never sleeps. Latency stubs, told apart by
argument, decide the winner:

```bynk,fragment
suite shop.prices {
  case "the faster mirror wins" {
    stub Prices.get("east", _) returns 1 after 2.seconds
    stub Prices.get("west", _) returns 2 after 1.seconds
    let p <- prices.call(7)
    expect p == 2
    expect Prices.get called 2 times     // the loser was still called
  }
}
```

Stubs replace capabilities, so a race between two *contexts* runs at `integration` or
`system`, not at `unit`. At `system`, `race` runs in real time. See
[testing](/book/reference/testing/#stub-latency).
