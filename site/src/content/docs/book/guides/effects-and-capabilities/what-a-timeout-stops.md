---
title: What a timeout stops
---
`e.within(d)` stops your handler waiting. It does not stop the work.

## The effect keeps going

When the deadline passes, `within` gives you `Err(TimedOut { after })` and the handler
moves on. The effect it abandoned is **not cancelled**. It is handed to the platform's
`waitUntil`, which keeps it alive after the handler returns, so it runs to completion:
a write is made, a commit in another agent happens, a message is sent.

So `Err(TimedOut)` does not mean "nothing happened". It means "I did not wait to find
out". If the call is not safe to repeat, a retry after a timeout may do the work twice.

## Why it is not cancelled

On Workers, a promise left running when the response returns is cancelled, possibly
halfway through a write. Dropping the abandoned effect would turn a slow call into a
half-finished one. Handing it to `waitUntil` lets it finish, and the cost is that you
cannot assume it did not.

## A race does not stop the loser either

[`a.race(b)`](/book/guides/effects-and-capabilities/ask-two-sources-and-take-the-first-answer/)
works the same way. The first effect to succeed wins, and the other is handed to
`waitUntil` and runs to completion, so its side effects still happen. Race work that is
safe to do twice.

## When the work must stop

Use the capability's own limit. `Fetch` has one: `FetchOptions.timeout` aborts the
request, and the result is `Err(Timeout)`. Nothing else in the capability layer can be
aborted yet.

## A fault after the timeout

If the abandoned effect faults after the deadline, nobody is left to catch it: the
handler has returned. The platform logs it. A fault *before* the deadline faults the
`within` call, as it would have faulted the plain call.

## Where this shows up in a test

A test settles the effects `within` abandoned when the call returns, as it settles `~>`
sends. A call that timed out is recorded when it was issued, like any call, so
`expect Pricing.quote called once` sees it.
