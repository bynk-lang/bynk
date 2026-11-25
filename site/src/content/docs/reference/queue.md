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
- **Parameters:** the message (conventionally named `message`), of any
  wire-deserialisable type, and optionally one `QueueDelivery` (see
  [Delivery metadata](#delivery-metadata)), in either order. Anything else is
  `bynk.queue.bad_params`. The
  message is deserialised from the queue body before the handler runs; a
  body that does not decode is never retried and the handler does not see it
  ([Messages that cannot be decoded](#messages-that-cannot-be-decoded)).
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
  [delivery policy](#delivery-policy) says). The reason is logged. It is redelivered
  after the consumer's `retryDelay` when `delivery { }` sets one, and at once otherwise.
- `RetryAfter(reason, delay)` — **retry after a delay**: the same, but redelivered
  after `delay` (a `Duration`) instead, which overrides `retryDelay` for this one
  message.

`RetryAfter` is how a handler backs off. With the attempt count from a
[`QueueDelivery`](#delivery-metadata), each retry can wait longer:
`RetryAfter("smtp unavailable", 10.seconds * delivery.attempts)`. Cloudflare retries
after a whole number of seconds, from 1 second to 24 hours, so a delay **written as a
literal** outside that range is `bynk.queue.retry_delay_out_of_range`, and a delay
**computed at run time** is rounded up to whole seconds (a part second rounds up, so the retry comes after at least the delay asked for) and clamped to the range, with
the clamp logged: a fault would retry with no delay, the opposite of what the handler
asked for. At the `system` tier a case reads the *settled* verdict: the delay the entry
gave Cloudflare, so a rounded or clamped delay reads as the one it became (`1500.milliseconds`
as `2.seconds`, `25.hours` computed as `24.hours`). `expect r == RetryAfter("down", 20.seconds)`
checks it, and `RetryAfter(_, d)` binds the delay. A `match` of a queue verdict names
`RetryAfter` or has a wildcard.

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

## Messages that cannot be decoded

Only the generated entry decodes a message body, so the handler never sees a body that does
not decode as its message type, and cannot decide what happens to it. The same bytes would
never decode on a retry, so the entry **never retries** one. What it does instead depends on
the consumer's `delivery { }`:

| `delivery` | A message that cannot be decoded |
|---|---|
| `deadLetter: "q-dlq"` | sent to `q-dlq` unchanged (as JSON), then acknowledged; one log line |
| `deadLetter: None`, or no section | acknowledged (dropped) and reported as a fault; one log line |

- The dead-letter send goes through a `[[queues.producers]]` binding the generated
  `wrangler.toml` adds for the consumer, named `__BYNK_DLQ_<QUEUE>` (the consumer's queue
  name in upper case, `-` as `_`). It is the consumer's own, so a Worker group's members and
  a context with several consumers never share one. If the send fails, or the binding is missing from `env`, the message is retried
  as any other fault is, so nothing is lost while the dead-letter queue is unavailable.
- The log line names the queue, the message type and the decode error's path and kind (for
  example `StructuralMismatch` at `$.id`), never the body, which can hold user data and is
  kept on the dead-letter queue for inspection.
- Without a dead-letter queue the fault is reported through `waitUntil`, so the invocation
  shows as faulted. Every message in the batch still gets an explicit `ack()` or `retry()`,
  which the platform honours over the rejected promise, so nothing is redelivered.
- A body that decoded before a consumer's message type changed fails now and is
  dead-lettered, which is correct: a message that cannot be read is not the handler's to run.

At the `system` tier the address yields `Undecodable` for such a body
([testing](/reference/testing/#cron-queue-system)).

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
