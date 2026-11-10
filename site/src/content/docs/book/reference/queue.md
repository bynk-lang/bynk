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
(`bynk.queue.duplicate_consumer`).

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
[emission](/docs/emission/) and
[Target Cloudflare Workers](/book/guides/projects-build-and-deployment/cloudflare-workers/).
