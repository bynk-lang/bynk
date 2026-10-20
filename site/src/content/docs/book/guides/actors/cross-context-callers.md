---
title: Know which context called you
---
**Goal:** in a cross-context `on call` handler, read *which* context made the
call.

When one context [consumes another's services](/book/guides/program-structure/consume-services/),
the call goes over a Cloudflare Service Binding — an internal, platform-dispatched
channel that is not reachable from the outside. Bynk trusts that channel (the
`Internal` scheme: "the channel itself is the assertion"), and the prelude actor
`Caller` yields the **calling context's name** as its identity.

```bynk
context billing

service charges {
  on call (amount: Int) -> Effect[Result[String, String]] by c: Caller {
    // c.identity : the qualified name of the context that called in,
    // e.g. "shop.orders"
    Ok(c.identity)
  }
}
```

`on call` defaults to `Caller`, so you only write the `by` clause when you want
to *capture* the name. A handler that omits it (or omits the binder) verifies the
channel but reads no caller — and is unchanged from before.

## How it works

- The **caller's name is stamped by the compiler** at the call site — a
  compile-time constant, not something the application sets — and travels beside
  the (unchanged) request body.
- The callee reads it at the boundary and threads it into the handler as
  `c.identity`. A call that does not identify its caller is rejected fail-closed.
- Verification is **static / channel-based** — no crypto. This establishes
  *identity*, not *authorisation*: it tells you who called, not whether they may.

`Caller` is admissible only on `on call` (cron takes `Scheduler`, queue takes
`Producer`).

**See also:** [Consume another context's services](/book/guides/program-structure/consume-services/),
[Reference — Actors](/book/reference/actors/).
