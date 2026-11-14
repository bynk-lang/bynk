---
title: Synchronous and asynchronous sends
---
A call is a **message to a recipient**. `Recipient.op(args)` sends `op(args)` to
`Recipient`, and you, the caller, decide whether to wait for the reply. That
choice is visible at the call site, in the statement you write.

## Two independent questions

When you call an effectful operation, two things are independent:

1. **Does the reply carry a value** you need?
2. **Do you need to wait** for it to complete?

It's tempting to think `Effect[()]` ("no value") means "no need to wait", but it
doesn't. A durable write like `Kv.putTtl(key, value, 86400)` returns `Effect[()]`
yet you *must* await it, or a later read in the same handler might not see it. A
webhook call also has a reply you may not care about, but you may not want the
response to wait for it either. Same type, different waiting.

So Bynk gives you a small grid, and a distinct spelling for each cell:

|              | **wait**         | **don't wait**         |
|--------------|------------------|------------------------|
| **use it**   | `let r <- e`     | — (not allowed)        |
| **discard it** | `do e`         | `do e.detach()`        |

- **`let r <- e`** is a *synchronous* call. The handler suspends, the reply comes
  back, and the next statement sees it. This is the everyday effect bind.
- **`do e`** waits for `e` and discards its reply. It is the durable-write case,
  where completion matters even though the value doesn't. `do` refuses a `Result`
  reply, because that would drop an `Err` without a trace; write `let _ <- e` to
  discard one on purpose.
- **`do e.detach()`** stops waiting. The handler carries on, nothing is bound, and
  the effect finishes after the response. Read it as "hand `e` to the runtime, and
  don't wait."

`detach` is an ordinary `Effect[()]`, so `let _ <- e.detach()` means the same as
`do e.detach()`, and it fits wherever an effect does: a helper's last line, or a
`forEach` step:

```bynk,fragment
fn notifyAll(reqs: List[Request]) -> Effect[()] given Fetch {
  reqs.forEach((r) => Fetch.send(r).detach())
}
```

This mirrors UML's message arrows: a filled arrowhead with a dashed return for a
synchronous call, an open arrowhead with no return for an asynchronous message.
Sequence diagrams draw `do e.detach()` as the open arrow.

There is no other spelling. A plain `let e = op(a)`, without the `<-`, does not
build a value you can await later: the call starts as soon as it is built, so it
would run unawaited, out of order with everything after it. Bynk rejects it
([`bynk.effect.unbound_effect`](/book/reference/diagnostics/)), as it does an
effect put into a list, an `Option` or a record. Bind it, sequence it with `do`,
detach it, or return it.

### `detach` stops waiting; it never starts anything

An effect is an eager promise: it starts where the expression is evaluated. So
`e.detach()` does not delay or start `e`; it only declines to wait for it. Values
`e` captured were read when `e` was built, and an effect that was never built (a
lambda that is never called) starts nothing. Where an effect is passed as an
argument, it has already started at the call site, and a `detach` in the callee
detaches a running effect.

### Logging does not need it

`Logger.info("…")` writes before it returns, so `do Logger.info("served")` costs
one microtask and nothing downstream waits on anything slow. Reach for `detach`
for work that really is slow: a webhook, a cache warm, a call through a provider
or helper you do not control.

## What happens when a detached effect fails

A detached effect has no caller to hand its failure to, so the runtime reports it,
and it is never lost. The compiler picks the route from the effect's type:

- **A fault** (the effect's promise rejects) goes to the platform's exception
  path, labelled with the `detach` site.
- **An `Err`**, when the effect is an `Effect[Result[_, E]]`, is logged as one
  structured `console.error` record naming the site, the error's type and, for a
  sum type, its variant. The payload is left out, because it can hold a
  credential.
- `Ok`, and any value that is not a `Result`, report nothing.

`Logger` is not involved, so the same code reports the same way with or without
`given Logger`. In a test the harness records each report and prints it under the
case, and `bynk test --format json` counts them as `detached_failures`; a report
does not fail the case, as on Workers. Each timeout of `e.within(d).detach()` is
reported as an `Err`, which is correct: the work was abandoned.

## Not in an agent handler

An agent handler can't detach. It runs inside the agent's Durable Object, not a
request, so there is no execution context to keep a detached effect alive after
the handler returns, and Bynk rejects the `detach`
([`bynk.detach.in_agent_handler`](/book/reference/diagnostics/)). Await the effect
instead with `let _ <- op(a)` (or `do op(a)`), or detach it from the service
handler that calls the agent.

A store operation gets its own refusal
([`bynk.detach.store_op`](/book/reference/diagnostics/)). A store write isn't a
message to anyone: it's staged and committed atomically with the rest of the
handler, so there is nothing to detach without waiting for. Write it as a plain
statement:

```bynk,fragment
do count.update((n) => n + 1).detach()   // ❌ a store write is part of the handler's commit
do count.update((n) => n + 1)            // ✅
```

## What it compiles to

On the Cloudflare Workers target a detached effect is handed to
[`ctx.waitUntil(…)`](https://developers.cloudflare.com/workers/runtime-apis/context/#waituntil),
so the runtime keeps the worker alive until the effect settles and the work
completes *after* your handler has returned its response rather than being
cancelled with it. That is what "don't wait, but still deliver" needs; a bare
un-awaited promise would simply be dropped.

The platform bounds that work: `waitUntil` can extend execution for up to 30
seconds after the response is sent or the client disconnects, shared across every
`waitUntil` call in the request, and promises still unsettled then are cancelled.
Detaching is immediate and best-effort. It is not retried, and it is not a
queue; use a queue for work that must survive.

On the Bundle target the compose root runs the effect and reports a failure, but
it does not keep a host alive: a Bundle host may exit before a detached effect
finishes.

## The removed spelling, `~>`

Before `detach`, the spelling was `~> e`. It is removed: a program that uses it is refused with
[`bynk.parse.removed_send_arrow`](/book/reference/diagnostics/), which names the replacement.
Write `do e.detach()`. On v0.603.x, `bynk check --fix` rewrites each one for you; see
[Upgrading](/book/reference/upgrading/). Unlike `~>`, `detach` accepts an effect of any type,
because a failure is reported rather than dropped.

**See also:** [Understand the capability model](/book/guides/effects-and-capabilities/understand-the-capability-model/),
[Reference: detaching an effect](/book/reference/capabilities/#detach).
