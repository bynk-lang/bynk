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
	to: String,
	subject: String,
}

service outbox from queue("outbound-email") {
	on message(message: EmailJob) -> Effect[QueueResult] by Producer {
		Ack
	}
}
```

The `message` parameter is deserialised from the queue body before the handler
runs, so the body always sees a valid value. A message that does not decode is
never retried, because the same bytes would fail again: it is sent to the
consumer's dead-letter queue, or dropped and reported when it has none (see
[Messages that cannot be decoded](/reference/queue/#messages-that-cannot-be-decoded)).

## Acknowledge or retry

The handler returns a `QueueResult` verdict: `Ack` acknowledges the message
(done); `Retry(reason)` redelivers it, logging the reason. You never call an ack
API — return the verdict and the framework routes it. The verdict is independent
of success or failure, so a poison message can be `Ack`'d to drop it:

```bynk,fragment
service outbox from queue("outbound-email") {
  on message(message: EmailJob) -> Effect[QueueResult] by Producer {
    Retry("smtp unavailable")
  }
}
```

A message that keeps retrying eventually reaches the retry limit (3 redeliveries by
default). Cloudflare then deletes it, unless you keep it, which is the next section.

## Back off

`Retry` redelivers at once, or after the delivery policy's `retryDelay`. A message that
failed because a dependency is down comes straight back and fails again, so return
`RetryAfter(reason, delay)` to wait. Take the attempt count from a `QueueDelivery` to
back off a little longer each time:

```bynk,fragment
service outbox from queue("outbound-email") {
  on message(job: EmailJob, delivery: QueueDelivery) -> Effect[QueueResult] by Producer given Smtp {
    let sent <- Smtp.send(job.to, job.subject)
    match sent {
      Ok(_) => Ack,
      Err(_) => RetryAfter("smtp unavailable", 30.seconds * delivery.attempts),
    }
  }
}
```

The delay is whole seconds from 1 second to 24 hours. A literal outside that is refused
(`bynk.queue.retry_delay_out_of_range`); a computed one is clamped, and logged when it is.

## Keep failed messages

Name a dead-letter queue in a `delivery { }` section, and a message that reaches
the retry limit is written there instead of deleted. `bynk deploy` creates the
queue, so there is nothing to set up in the dashboard, which a deploy would
overwrite anyway:

```bynk,fragment
service outbox from queue("outbound-email") {
  delivery {
    retries: 5,
    retryDelay: 30.seconds,
    deadLetter: "outbound-email-dlq",
  }

  on message(message: EmailJob) -> Effect[QueueResult] by Producer {
    Retry("smtp unavailable")
  }
}
```

The dead-letter queue is an ordinary queue, so a consumer of its own can inspect or
replay what failed. It takes the same message type:

```bynk,fragment
service review from queue("outbound-email-dlq") {
  delivery {
    deadLetter: None,
  }

  on message(message: EmailJob) -> Effect[QueueResult] by Producer given Logger {
    let _ <- Logger.warn("gave up on an email to \(message.to)")
    Ack
  }
}
```

A consumer with no `deadLetter` draws a warning, because the platform default
deletes the message. Write `deadLetter: None` when that is what you want. See the
[delivery policy](/reference/queue/#delivery-policy) for every key.

## Handle a redelivered message

Delivery is at least once, so a handler can see a message again, after a `Retry` or in
rare cases without one. Take the delivery metadata as a second parameter to back off by
attempt or to notice a message that keeps failing:

```bynk,fragment
consumes bynk.cloudflare { QueueDelivery }

service outbox from queue("outbound-email") {
  on message(message: EmailJob, delivery: QueueDelivery) -> Effect[QueueResult] by Producer given Logger {
    if delivery.attempts > 3 {
      let _ <- Logger.warn("giving up on \(message.id) after \(delivery.attempts) attempts")
      Ack
    } else {
      Retry("attempt \(delivery.attempts)")
    }
  }
}
```

`attempts` counts the times this consumer has been given the message, starting at 1, and
`sentAt` is when it was sent. Neither deduplicates: to do that, put a key in the message
when you send it, and claim it with `Idempotency.claim` before doing the work, so a
second delivery with the same key does nothing.

## Use a capability

A queue handler reaches the outside world through `given`, like any handler:

```bynk,fragment
  on message(message: EmailJob) -> Effect[QueueResult] by Producer given Smtp {
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
`Retry`. See [Cron and queue at `system`](/reference/testing/#cron-queue-system).

## Send to the queue

A context sends messages with a `bynk.cloudflare` `Queue` instance that names
the queue. `sendValue` encodes the value for you, and when a context in the project
consumes the queue, the compiler checks that the value is the type the consumer takes:

```bynk,fragment
capability Outbound = Queue @name("outbound-email")

service signup from http {
  on POST("/welcome") (to: String) -> Effect[HttpResult[String]] by Visitor given Outbound {
    let _ <- Outbound.sendValue(EmailJob { to: to, subject: "Welcome" })
    Accepted("queued")
  }
}
```

Sending a type the consumer cannot read is `bynk.queue.message_type_mismatch`, caught
at compile time rather than as a message that is retried and dead-lettered. `send` takes
JSON text instead, which the compiler cannot check, so it warns when the project consumes
the queue.

See [Sending to a queue](/reference/bynk-capabilities/#sending-to-a-queue-queue) and
[Producers and consumers agree](/reference/queue/#producers-and-consumers-agree).

## Build and run

Queue services compile to a Cloudflare Worker with `--target workers`; each
queue becomes a `[[queues.consumers]]` binding in the generated
`wrangler.toml`. See
[Target Cloudflare Workers](/book/guides/projects-build-and-deployment/cloudflare-workers/).

## Related

- Reference: [Queue](/reference/queue/).
- Reference: [Cron](/reference/cron/) — the sibling time-driven handler.
