---
title: Deduplicate at-least-once deliveries
---
**Goal:** run the work for a webhook, a queue message or a retried command at most
once, even when the sender delivers it twice or two copies arrive together.

Senders that promise delivery promise it *at least once*: a webhook is resent when
your answer is slow, a queue message is redelivered when a consumer is cut off, a
client retries a command it never saw an answer to. The same delivery can reach you
more than once, and two copies can arrive at the same moment.
[`Idempotency.claim`](/book/reference/bynk-capabilities/#the-idempotency-capability)
is the one call that makes "the first delivery wins" correct under that.

## Claim the delivery

Pick the id the sender gives every delivery of one event (a delivery id, a message
id, an idempotency key), claim it, and do the work only if you were first:

```bynk
context hooks

consumes bynk { Idempotency, Claim }

type Event = {
	id: String,
}

actor Webhook { auth = Signature(secret = "WEBHOOK_SECRET", header = "X-Signature") }

service api from http {
	on POST("/hooks/event") (
		body: Event
	) -> Effect[HttpResult[String]] by Webhook given Idempotency {
		let Claim.Claimed <- Idempotency.claim(body.id, 7.days) else { Ok("duplicate") }
		// ... do the work here: it runs at most once per delivery id ...
		Ok("processed")
	}
}
```

`claim` returns `Claimed` to exactly one caller for a key, and `AlreadyClaimed` to every
other while the claim lasts (here, seven days, so a sender that retries for a week is
covered). The `else` arm answers the repeats. Answer them `200`, not an error: a
sender that sees an error retries, then may disable the webhook.

## What `claim` guarantees

- **At most one `Claimed` per key.** On Cloudflare that holds across isolates and
  restarts: the claim is stored in a D1 database the build declares for you, which
  `bynk deploy` creates and migrates and the plan lists as
  `d1 create <worker>-idempotency (D1_IDEMPOTENCY)`. You write no SQL. On Node and in the
  browser the claim is in memory, so it holds within one process only.
- **A failed handler gives the claim back.** If the handler faults, returns an `Err`, or
  answers with a `5xx`, the claim is released, so the sender's retry is processed. A
  `4xx` or any other answer is a completed outcome and keeps the claim.
- **A claim is a 30-second lease until the handler finishes.** If the Worker is cut
  off before it can release, the claim lapses after 30 seconds and a retry can claim
  again. A handler that runs longer than that can be claimed again while it is still
  running.

That is **at-most-once completion, not exactly-once side effects**. Work done before a
failure, such as a call to another service, can happen again on the retry, so make
those calls idempotent too (pass the same delivery id along as their key).

## Choose the key

The key you pass is scoped to the calling handler, so two handlers can use the same
literal key without meeting. Within one handler, two different senders or tenants that
could use the same id need a key that tells them apart: claim `"\(tenant):\(body.id)"`,
not `body.id`.

## When you need the earlier answer

`claim` records no outcome, so a repeat is told only that it is a repeat. To give a repeat
the *same answer* as the first delivery, use `dedup` and `remember`, which cache an
outcome under a key. They are a plain check and then a write, so two simultaneous
deliveries can both miss. The value must be `Codable`; on Cloudflare it is stored in the
same D1 database as claims, so it survives restarts and is seen by every instance, and a
stored value that no longer decodes counts as a miss. Where the work must run once, claim
first.

## See also

- [Verify webhooks](/book/guides/actors/verify-webhooks/): the signature check that
  runs before the handler, and the replay window it enforces.
- [The `Idempotency` capability](/book/reference/bynk-capabilities/#the-idempotency-capability).
- [Deploy to Cloudflare](/book/guides/projects-build-and-deployment/deploy-to-cloudflare/#the-idempotency-database):
  the database `claim` uses.
