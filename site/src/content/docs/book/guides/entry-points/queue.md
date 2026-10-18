---
title: Process a queued message
---
**Goal:** consume messages from a queue — sending emails, processing uploads,
fanning out work — one message at a time, with automatic retry on failure.

Queue handlers go in a `service` inside a `context`. The queue is bound on the
service header (`from queue("name")`); each `on message` handler takes the
message as its single parameter and returns `Effect[QueueResult]`.

## Consume a message

```bynk
context mailer

type EmailJob = {
  to:      String,
  subject: String,
}

service outbox from queue("outbound-email") {
  on message(message: EmailJob) -> Effect[QueueResult] {
    Ack
  }
}
```

The `message` parameter is deserialised from the queue body before the handler
runs — a malformed message is retried, so the body always sees a valid value.

## Acknowledge or retry

The handler returns a `QueueResult` verdict: `Ack` acknowledges the message
(done); `Retry(reason)` redelivers it, logging the reason. You never call an ack
API — return the verdict and the framework routes it. The verdict is independent
of success or failure, so a poison message can be `Ack`'d to drop it:

```bynk,fragment
service outbox from queue("outbound-email") {
  on message(message: EmailJob) -> Effect[QueueResult] {
    Retry("smtp unavailable")
  }
}
```

A message that keeps retrying eventually hits the queue's dead-letter policy
(configured outside Bynk).

## Use a capability

A queue handler reaches the outside world through `given`, like any handler:

```bynk,fragment
  on message(message: EmailJob) -> Effect[QueueResult] given Smtp {
    let _ <- Smtp.send(message.to, message.subject)
    Ack
  }
```

## Test it

A test case delivers a message with `svc.message(m)`, which yields the
handler's `QueueResult`:

```bynk,fragment
suite mailer as system {
  case "an email job is acknowledged" {
    let r <- outbox.message(EmailJob { to: "ada@example.com", subject: "Hi" })
    expect r is Ack
  }
}
```

At `system` the message is serialised, sent through a JSON round trip, and
delivered through the deployable Worker's own `queue` entry point, as the
platform delivers it. A consumer that faults is retried, so the address yields
`Retry`. See [Cron and queue at `system`](/book/reference/testing/#cron-queue-system).

## Build and run

Queue services compile to a Cloudflare Worker with `--target workers`; each
queue becomes a `[[queues.consumers]]` binding in the generated
`wrangler.toml`. See
[Target Cloudflare Workers](/book/guides/projects-build-and-deployment/cloudflare-workers/).

## Related

- Reference: [Queue](/book/reference/queue/).
- Reference: [Cron](/book/reference/cron/) — the sibling time-driven handler.
