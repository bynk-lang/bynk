---
title: Know which context called you
---
**Goal:** in a cross-context `on call` handler, read *which* context made the
call.

When one context [consumes another's services](/book/guides/program-structure/consume-services/),
the call goes over a Cloudflare Service Binding. Every call is **signed**: the
caller signs it with a key it shares with the callee (one key per pair of
contexts), and the callee checks the signature before it looks at anything else.
The prelude actor `Caller` yields the **calling context's name**, now proved by
that signature, as its identity.

```bynk
context billing

service charges {
	on call(amount: Int) -> Effect[Result[String, String]] by c: Caller {
		// c.identity : the qualified name of the context that called in,
		// e.g. "shop.orders"
		Ok(c.identity)
	}
}
```

An `on call` handler always states its actor (`bynk.actor.missing_by_on_call`);
`by Caller` is the explicit "any consuming context". Write the binder (`by c: Caller`)
only when you want to *capture* the name. `by Caller` without a binder checks the
signature but reads no caller.

## How it works

- The **caller's name is stamped by the compiler** at the call site, a
  compile-time constant that the application cannot set, and travels beside the
  (unchanged) request body.
- The callee verifies the request's signature under the key it holds for that
  caller, then reads the name the signature covers. A call that does not verify
  is refused (`401`) before any handler runs.
- This establishes *identity*; *authorisation* is the actor's job. `by Caller`
  admits every context that consumes the service, and the next section narrows it.

`Caller` is admissible only on `on call` (cron takes `Scheduler`, queue takes
`Producer`).

## Restrict who may call

A service meant for one or two contexts should say so in its signature, not in a
string comparison in the body. A **refinement over `Caller`** names the calling
contexts, with the `IsContext` predicate:

```bynk,ignore
context shop.billing

actor Admin      = Caller where IsContext(shop.admin)
actor Fulfilment = Caller where IsContext(shop.orders, shop.fulfil)

service refunds {
	on call(id: String) -> Effect[Result[(), String]] by Admin { … }
}

service ship {
	on call(order: String) -> Effect[Result[(), String]] by c: Fulfilment {
		// c.identity : the calling context, one of the two
		…
	}
}
```

The context names are written as names, not strings, and each must be a context
of the project (`bynk.actor.unknown_context`). `!IsContext(shop.legacy)` reads as
a deny-list, and the usual `&&` and `||` combine.

The compiler sees every call in the project, so a call from a context the
predicate does not admit is refused **where it is written**, before anything is
deployed (`bynk.actor.caller_not_admitted`). The callee still checks the proven
caller when the request arrives and answers a context it does not admit with
`403`, because signing keys are per pair of contexts and a caller from before the
restriction may still be deployed.

A refinement over `Caller` can be a sum's one `Internal` member, so
`by who: Fulfilment | Ledger` admits both a context and a declared foreign
caller. In a test, a call-site `by Admin("shop.admin")` acts as the refinement,
and naming a context the predicate refuses is `bynk.actor.caller_not_admitted`
in the case too. See [authorisation](/book/guides/actors/authorisation/) for the
`Bearer` form.

**See also:** [Consume another context's services](/book/guides/program-structure/consume-services/),
[Reference — Actors](/reference/actors/).
