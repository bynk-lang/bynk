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
  It must be a name Cloudflare accepts: 1 to 63 lowercase letters, digits or `-`,
  starting and ending with a letter or digit (`bynk.queue.invalid_name`).
- **Parameter:** exactly one — the message (conventionally named `message`), of
  any wire-deserialisable type. Anything else is `bynk.queue.bad_params`. The
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
- `Retry(reason)` — **retry** the message (it is redelivered; a message that
  reaches the retry limit is dead-lettered or deleted, as the
  [delivery policy](#delivery-policy) says). The reason is logged.

The handler never calls an ack API itself — it returns a verdict and the
framework routes it. The verdict is **independent of success/failure**: `Ack` a
logical failure to drop a poison message, or `Retry` despite partial success.
This is why queue handlers return `QueueResult` rather than `Result[(), E]`
(the agency rule, ADR 0078), the same way an HTTP handler names its wire outcome
with `HttpResult`.

## Delivery policy

A `delivery { }` section on the service sets how the consumer's messages are
retried, delayed, dead-lettered and batched. It is written into the consumer's
`[[queues.consumers]]` in the generated `wrangler.toml`, which Bynk owns, so the
source is the one place to set it: a value changed in the dashboard is overwritten
by the next `bynk deploy`.

```bynk
context mailer

type EmailJob = {
	to: String,
}

const Backoff: Duration = 30.seconds

service outbox from queue("outbound-email") {
	delivery {
		retries: 5,
		retryDelay: Backoff,
		deadLetter: "outbound-email-dlq",
		batchSize: 25,
		batchWait: 10.seconds,
		concurrency: 4,
	}

	on message(job: EmailJob) -> Effect[QueueResult] {
		Ack
	}
}
```

Every key is optional, and an omitted key keeps Cloudflare's default (Bynk's
`batchSize` default is 10).

| Key | Type | Range | Cloudflare default | `wrangler.toml` |
|---|---|---|---|---|
| `retries` | `Int` | 0 to 100 | 3 | `max_retries` |
| `retryDelay` | `Duration` | whole seconds, at most 24 hours | none | `retry_delay` |
| `deadLetter` | `String`, or `None` | a queue name | none | `dead_letter_queue` |
| `batchSize` | `Int` | 1 to 100 | 10 | `max_batch_size` |
| `batchWait` | `Duration` | whole seconds, 0 to 60 | 5 seconds | `max_batch_timeout` |
| `concurrency` | `Int` | 1 to 250 | unset (scales to the maximum, which Cloudflare recommends) | `max_concurrency` |

A literal, or a constant of the key's type, is accepted ([constants](/reference/constants/)).
A value outside its range, an unknown key or a repeated key is
`bynk.queue.delivery_bad_option`, and a second `delivery` section is
`bynk.parse.duplicate_delivery`. The section is only valid on a `from queue`
service (`bynk.queue.delivery_not_queue`).

### Dead-letter queue

Delivery is **at least once** and ordering is best effort. Cloudflare redelivers a
message that was retried, up to `retries` times, and then **deletes it** unless the
consumer has a dead-letter queue, in which case it writes it there instead. So a
consumer with no `deadLetter` draws the warning `bynk.queue.no_dead_letter`.
Writing `deadLetter: None` says deleting such a message is intended, and silences it.

The dead-letter queue is an ordinary queue:

- its name must be one Cloudflare accepts, and not the consumer's own queue;
- `bynk deploy` appends `-<environment>` to it as it does to the consumer's queue
  (outside `default`) and creates it if it is missing, recording it in
  `bynk.deploy.lock` only when Bynk created it, as for a queue the context only
  sends to;
- a Bynk consumer may read it (`from queue("outbound-email-dlq")`), and that
  consumer's message type must be the source consumer's
  (`bynk.queue.dead_letter_type_mismatch`), since the dead letter is the same body.

The environment-qualified name of any queue, consumer, producer or dead-letter,
must still fit Cloudflare's 63 characters. Only the deploy knows the environment,
so a name that overflows once `-<environment>` is added is refused by the deploy
plan, before anything is created.

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
`[[queues.consumers]]` binding in the generated `wrangler.toml`, carrying the
[delivery policy](#delivery-policy)'s settings. See
[emission](/reference/tooling/emission/) and
[Target Cloudflare Workers](/book/guides/projects-build-and-deployment/cloudflare-workers/).
