---
title: Synchronous and asynchronous sends
---
A call is a **message to a recipient**. `Recipient.op(args)` sends `op(args)` to
`Recipient` — and you, the caller, decide whether to wait for the reply. That
choice is visible at the call site, in the marker you write.

## Two independent questions

When you call an effectful operation, two things are independent:

1. **Does the reply carry a value** you need?
2. **Do you need to wait** for it to complete?

It's tempting to think `Effect[()]` ("no value") means "no need to wait" — but it
doesn't. A durable write like `Kv.putTtl(key, value, 86400)` returns `Effect[()]`
yet you *must* await it, or a later read in the same handler might not see it. A
log line like `Logger.info("…")` also returns `Effect[()]`, but nothing downstream
depends on it finishing. Same type, different waiting.

So Bynk gives you a small grid, and a distinct spelling for each cell:

|              | **await**            | **don't await**          |
|--------------|----------------------|--------------------------|
| **value**    | `let r <- op(a)`     | — (not allowed)          |
| **no value** | `let _ <- op(a)`     | `~> op(a)`               |

- **`let r <- op(a)`** — a *synchronous* call with a valued reply. The handler
  suspends, the value comes back, and the next statement sees it. This is the
  everyday effect bind.
- **`let _ <- op(a)`** — synchronous, but you discard the (empty) reply. You still
  wait: this is the durable-write case, where completion matters even though the
  value doesn't.
- **`~> op(a)`** — an *asynchronous send*: fire it and move on. No reply, no
  waiting, nothing bound. Read it as "send `op(a)` to its recipient, don't wait."

(If you reach for the missing top-right cell — a valued reply you don't want to
wait for — Bynk asks you to be honest and write `let _ <- op(a)`: await it and
discard the value, rather than silently dropping data.)

This mirrors UML's message arrows: a filled arrowhead with a dashed return for a
synchronous call, an open arrowhead with no return for an asynchronous message.

There is no fourth spelling. A plain `let e = op(a)`, without the `<-`, does not
build a value you can await later: the call starts as soon as it is built, so it
would run unawaited, out of order with everything after it. Bynk rejects it
([`bynk.effect.unbound_effect`](/book/reference/diagnostics/)), as it does an
effect put into a list, an `Option` or a record. Bind it, sequence it with `do`,
send it with `~>`, or return it.

## The error gate

`~>` throws away the reply, so Bynk only lets you use it when there is nothing
worth keeping — the operation must return `Effect[()]`. A send to an operation
that returns a real value or an error is rejected
([`bynk.send.requires_unit`](/book/reference/diagnostics/)), because that value
or error would vanish without a trace:

```bynk,fragment
~> Logger.info("served")                  // ✅ Effect[()] — nothing to drop
~> Fetch.send(request)                     // ❌ Effect[Result[Response, FetchError]]
                                           //    the error would be lost; use `let r <- …`
```

The contract bounds what you *may* do; you still choose whether to wait.

## Not in an agent handler

An agent handler can't send. It runs inside the agent's Durable Object, not a
request, so there is no execution context to keep a fire-and-forget effect alive
after the handler returns, and Bynk rejects the `~>`
([`bynk.send.in_agent_handler`](/book/reference/diagnostics/)). Await the effect
instead with `let _ <- op(a)` (or `do op(a)`), or send it from the service handler
that calls the agent.

A store operation gets its own refusal
([`bynk.send.store_op`](/book/reference/diagnostics/)). A store write isn't a
message to anyone: it's staged and committed atomically with the rest of the
handler, so there is nothing to send without waiting for. Write it as a plain
statement:

```bynk,fragment
~> count.update((n) => n + 1)   // ❌ a store write is part of the handler's commit
do count.update((n) => n + 1)   // ✅
```

## What it compiles to

On the Cloudflare Workers target a send becomes
[`ctx.waitUntil(…)`](https://developers.cloudflare.com/workers/runtime-apis/context/#waituntil) —
the runtime keeps the worker alive until the effect settles, so the send
completes *after* your handler has returned its response rather than being
cancelled with it. That is exactly what "fire-and-forget but actually deliver"
needs; a bare un-awaited promise would simply be dropped.

## When to reach for it

Today the natural fit is logging and metrics — best-effort, out-of-band work the
response shouldn't block on. As Bynk grows event emission, push notifications, and
queue sends, those one-way channels will share this same call-site form, so the
distinction you read in `~>` stays the same wherever it appears.

**See also:** [Understand the capability model](/book/guides/effects-and-capabilities/understand-the-capability-model/),
[Reference — effect statements](/book/reference/grammar/#rule-effect_send_stmt).
