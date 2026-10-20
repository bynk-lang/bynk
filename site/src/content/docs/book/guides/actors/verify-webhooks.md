---
title: Verify an inbound webhook
---
**Goal:** accept a webhook (from Stripe, GitHub, a sibling system) only when its
signature proves it came from the trusted sender — and reject replays.

A webhook is authenticated not by a token but by a **signature over the request
body**: the sender computes an HMAC of the raw body with a shared secret, and you
recompute and compare it. A `Signature` actor makes the compiler emit that check.

## A signed webhook

```bynk
context hooks

type Event = { id: String }

actor Webhook {
  auth = Signature(secret = "WEBHOOK_SECRET", header = "X-Signature")
}

service api from http {
  on POST("/hooks/event") (body: Event) -> Effect[HttpResult[String]] by Webhook {
    Ok("received")
  }
}
```

Before the body runs, the boundary reads the raw request body **once**,
recomputes HMAC-SHA256 over those exact bytes (WebCrypto, constant-time), and
compares it against the configured header — accepting a bare hex digest or a
`sha256=<hex>` prefix (the GitHub shape). Any mismatch, or a missing/malformed
header, **fails closed with `401`**; only then is the `body` parsed, from the
same bytes. There is no app-written HMAC, and no re-serialisation to get the
bytes wrong.

A `Signature` actor:
- attests **authenticity, not a principal** — it has no `identity`, so the
  binder is always omitted (`by Webhook`);
- **must take a `body`** — the signature is over the body, so a bodyless signed
  request is meaningless;
- is **HTTP-only**.

## Reject replays with a signed timestamp

Webhooks retry, so a captured request can be replayed. When the sender signs a
timestamp, configure a `timestamp` header and a `tolerance` (in seconds):

```bynk
context hooks

type Event = { id: String }

actor Webhook {
  auth = Signature(
    secret = "WEBHOOK_SECRET",
    header = "X-Signature",
    timestamp = "X-Timestamp",
    tolerance = 300
  )
}

service api from http {
  on POST("/hooks/event") (body: Event) -> Effect[HttpResult[String]] by Webhook {
    Ok("received")
  }
}
```

Now the signed string is `<timestamp>.<body>`, and the boundary additionally
rejects (fail-closed) a request whose timestamp is not a finite number within
`tolerance` seconds of now — a five-minute replay window here. A `tolerance`
without a `timestamp` is a compile error.

The replay window *bounds* replay; it does not *eliminate* it. Full event
deduplication (by event id) is the job of an idempotency capability — out of
scope here.

## Test the webhook end to end

A `system`-tier case runs the context as the Worker it deploys as, so the real
seam checks every request. The test driver signs each call to a `Signature`
route as the actor declares, over exactly the bytes it sends, so a plain call is
accepted. Three reserved principals send what the seam must refuse:

```bynk,ignore
suite hooks as system {
  env { WEBHOOK_SECRET = "test-hook" }   // optional: else a fixed test secret

  case "a signed event is accepted, and a replay too" {
    let a <- api.POST("/hooks/event", Event { id: "evt_1" })
    let b <- api.POST("/hooks/event", Event { id: "evt_1" })
    expect a is Ok("received")
    expect b is Ok("received")
  }

  case "unsigned, forged and stale events are refused" {
    let e = Event { id: "evt_1" }
    let none <- api.POST("/hooks/event", e) by Nobody    // no signature
    let forged <- api.POST("/hooks/event", e) by Forged  // a wrong secret
    let stale <- api.POST("/hooks/event", e) by Stale    // tolerance + 60s old
    expect none is Rejected(Unauthorized)
    expect forged is Rejected(Unauthorized)
    expect stale is Rejected(Unauthorized)
  }
}
```

`by Stale` needs the `timestamp` and `tolerance` of the second actor above. A
`Wire("…")` body is signed as written, so a case can also check a signed but
malformed body. See [the failure
principals](/book/reference/testing/#failure-principals).

**See also:** [Reference — Actors](/book/reference/actors/).
