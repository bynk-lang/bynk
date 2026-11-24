---
title: Test a handler that calls another context
---
**Goal:** test a handler that calls a service in another context, including the
failure paths, without deploying the other context.

A handler that calls `Pay.authorise(…)` crosses a context boundary. Below
`system` no other context is stood up, so the case would have nothing to call
(`bynk.tier.cross_context_needs_system`). A [`stub`](/reference/testing/#stub-services)
on the consumed service substitutes it, as a stub substitutes a capability.

## Stub the service

```bynk,fragment
suite shop.orders {
	consumes shop.pay { Receipt, PaymentError }

	case "an order is placed when payment succeeds" {
		stub Stock.reserve(_) returns Ok(())
		stub Pay.authorise(_) returns Ok(Receipt { id: "r1" })
		let r <- placeOrder.call("widget", 100)
		expect r is Ok(_)
		expect Pay.authorise called once
	}
}
```

The left side names the service the way the handler does: through the alias the
file gives the context (`consumes shop.pay as Pay`), or its path
(`shop.pay.authorise(…)`). The right side is typed against the service's declared
return type, so if `shop.pay` changes what `authorise` returns, this stub stops
compiling.

## Test the failure paths

Compensation and error handling are the paths a real deployment rarely lets you
produce on demand. A stub does:

```bynk,fragment
case "a declined payment releases the stock" {
	stub Stock.reserve(_) returns Ok(())
	stub Pay.authorise(_) returns Err(Declined)
	stub Stock.release(_) returns Ok(())
	let r <- placeOrder.call("widget", 100)
	expect r is Err(_)
	expect Stock.release called once
}

case "the first payment fails, the retry succeeds" {
	stub Pay.authorise(_) returns each [Err(Down), Ok(Receipt { id: "r2" })]
	let first <- placeOrder.call("widget", 100)
	let second <- placeOrder.call("widget", 100)
	expect first is Err(_)
	expect second is Ok(_)
}

case "a payment outage is a fault" {
	stub Pay.authorise(_) fails
	expect placeOrder.call("widget", 100) faults
}
```

`returns Err(…)` is the service's in-band failure, `fails` is a fault in the call
itself, and `returns each` answers successive calls differently. `after` and `never`
delay or withhold the answer, as for a capability.

## What a stub does not hide

- A service the case reaches **without** a stub is still
  `bynk.tier.cross_context_needs_system`, and the error names it, so a case never
  runs a real cross-context call in-process by accident.
- At `integration` a stubbed call's arguments and result also cross the consumed
  context's wire codecs, so a value the wire cannot carry (a `Float` that is `NaN`,
  bare or nested, say) fails the call there although it passes at `unit`.
- A stub checks your handler against the service's *declared* signature, not its
  behaviour. A `system` suite, which stands the contexts up and calls over the
  real wire, remains the end-to-end check. See
  [Test tiers](/book/guides/testing/integration/).

**See also:** [Reference — `stub`](/reference/testing/#stub-services),
[Reference — observation](/reference/testing/#observation).
