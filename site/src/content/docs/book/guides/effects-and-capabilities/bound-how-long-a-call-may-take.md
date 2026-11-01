---
title: Bound how long a call may take
---
**Goal:** stop a handler waiting on a slow dependency for longer than it can afford,
and say what it does instead.

## Write `within`

Add `.within(d)` to any effect. The result is a `Result` that is `Err(TimedOut)` when
the deadline passes first:

```bynk
context shop.quote

consumes bynk

capability Pricing {
  fn quote(id: Int) -> Effect[Int]
}

service quotes {
  on call(id: Int) -> Effect[String] given Pricing {
    let r <- Pricing.quote(id).within(2.seconds)
    match r {
      Ok(q) => Effect.pure("quoted \(q)")
      Err(TimedOut { after, .. }) => Effect.pure("the quote is taking too long")
    }
  }
}
```

`TimedOut` belongs to `bynk`, so the context needs `consumes bynk` (or `consumes bynk { TimedOut }`). `within` works on any
`Effect`, so it needs no support from the capability behind it.

If the effect already returns a `Result`, you get one more layer, and you unwrap both:

```bynk,fragment
let r <- Pricing.tryQuote(id).within(3.seconds)
match r {
  Ok(Ok(q)) => Effect.pure(true)
  Ok(Err(why)) => Effect.pure(false)
  Err(late) => Effect.pure(false)
}
```

## Bound several calls at once

A lambda written in the handler can use `within`, and a `par` block bounds each of its
bindings alone:

```bynk,fragment
let rs <- ids.parTraverseAll((id) => Pricing.quote(id).within(1.seconds))
```

## Where it is allowed

Only in a **service handler's body**, lambdas included. An agent handler is refused
(`bynk.within.in_agent_handler`), and so is a function or a provider operation
(`bynk.within.outside_handler`), and so is a WebSocket handler: they have no
execution context to hand a late effect to. Call `within` in the handler, and pass the function the bounded effect.

## Test the timeout

A test runs `within` on virtual time, so it never sleeps. A stub can take time, or never
finish:

```bynk,fragment
suite shop.quote {
  case "a slow quote times out" {
    stub Pricing.quote(_) returns 7 after 3.seconds
    let r <- quotes.call(1)
    expect r == "the quote is taking too long"
  }

  case "a quote that never arrives times out" {
    stub Pricing.quote(_) never
    let r <- quotes.call(1)
    expect r == "the quote is taking too long"
  }
}
```

The harness moves its clock to the deadline for you, but only while every call in flight
is a stub waiting on `after` or `never`; a call into a real provider is waited for. See
[testing](/book/reference/testing/#stub-latency).

For what a timeout does and does not stop, read
[what a timeout stops](/book/guides/effects-and-capabilities/what-a-timeout-stops/).
