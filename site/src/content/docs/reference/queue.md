---
title: Queue
---
Queue handlers consume messages from a named queue. Like HTTP and cron
handlers, they are declared in a `service` inside a `context`; the queue is
bound on the service header (`from queue("name")`), and each handler is an
`on message(…)`.

## Handler form

```bynk,ignore
service <Name> from queue("<name>") {
  on message(message: T) -> Effect[QueueResult] {
    …
  }
}
```

- **Name:** the queue this consumer binds to, on the `from queue("…")` header.
  It must be non-empty (`bynk.queue.invalid_name`).
- **Parameters:** the message (conventionally named `message`), of any
  wire-deserialisable type, and optionally one `QueueDelivery` (see
  [Delivery metadata](#delivery-metadata)), in either order. Anything else is
  `bynk.queue.bad_params`. The
  message is deserialised from the queue body before the handler runs; a
  malformed message is retried.
- **Return type:** must be `Effect[QueueResult]` — the verdict sum `Ack | Retry`
  (`bynk.queue.return_not_queue_result`). `Ack` confirms the message; `Retry`
  redelivers it, carrying a `String` reason for the log.
- **Placement:** only inside a `service`, never an `agent`
  (`bynk.parse.handler_in_agent`).

No two queue handlers in a context may consume the same queue
(`bynk.queue.duplicate_consumer`). The same holds across the project: Cloudflare
accepts one consumer Worker per queue, so two contexts that consume one queue are
refused too, whether or not they share a `[deploy.workers]` group. Any number of
contexts may *send* to a queue; only the consumer is limited to one.

## Acknowledgement and retry

Each message is acknowledged or retried by the handler's `QueueResult` verdict:

- `Ack` — **acknowledge** the message; it is removed from the queue.
- `Retry(reason)` — **retry** the message (it is redelivered; persistent
  failures hit the queue's dead-letter policy). The reason is logged.

The handler never calls an ack API itself — it returns a verdict and the
framework routes it. The verdict is **independent of success/failure**: `Ack` a
logical failure to drop a poison message, or `Retry` despite partial success.
This is why queue handlers return `QueueResult` rather than `Result[(), E]`
(the agency rule, ADR 0078), the same way an HTTP handler names its wire outcome
with `HttpResult`.

## Delivery metadata

A handler may take what Cloudflare knows about the delivery as a second parameter, of
type `QueueDelivery` (`consumes bynk.cloudflare { QueueDelivery }`). It is identified by
its type, so it may come before or after the message:

```bynk,fragment
service outbox from queue("outbound-email") {
	on message(job: EmailJob, delivery: QueueDelivery) -> Effect[QueueResult] {
		if delivery.attempts > 3 {
			Ack
		} else {
			Retry("attempt \(delivery.attempts)")
		}
	}
}
```

| Field | Type | Meaning |
|---|---|---|
| `attempts` | `Int` | The times the consumer has been given the message, starting at 1. |
| `sentAt` | `Instant` | When the message was sent, so a message sent with a delay has waited at least that long. |

Delivery is **at least once**: a message can be delivered more than once, so neither
field is a deduplication key. Only a key the producer writes into the message
deduplicates, and the guide shows it with `Idempotency.claim`. Cloudflare also gives each
message an `id`, but whether it is the same across redeliveries is not documented, so
Bynk does not expose it until that has been shown on a deployed queue.

A message whose own type is `QueueDelivery`, or a second `QueueDelivery`, is
`bynk.queue.bad_params`.

## Example

```bynk
context mailer

type EmailJob = {
	to: String,
	subject: String,
}

service outbox from queue("outbound-email") {
	on message(message: EmailJob) -> Effect[QueueResult] {
		Ack
	}
}
```

## Emission

`on message` handlers compile to the Worker's `queue` entry point on the
`--target workers` target (dispatching on `batch.queue`, deserialising each
message, acking on `Ack` / retrying on `Retry`), and every queue becomes a
`[[queues.consumers]]` binding in the generated `wrangler.toml`. See
[emission](/reference/tooling/emission/) and
[Target Cloudflare Workers](/book/guides/projects-build-and-deployment/cloudflare-workers/).
