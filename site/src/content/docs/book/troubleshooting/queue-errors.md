---
title: "`bynk.queue.*` errors"
---
`on message` handlers consume one message at a time and have a fixed shape: a
non-empty queue name, exactly one message parameter, and an
`Effect[Result[(), E]]` return. These are the common errors when that shape is
broken.

## `bynk.queue.bad_params`

```text
[bynk.queue.bad_params] `on message` handlers take exactly one parameter (the message)
```

**Cause:** a queue handler declared zero, or more than one, parameter. A queue
consumer processes exactly one message per invocation.

**Fix:** declare a single parameter (conventionally `message`) of the message's
type.

## `bynk.queue.invalid_name`

```text
[bynk.queue.invalid_name] `from queue` requires a non-empty queue name
```

**Cause:** the queue name string is empty, or is not a name Cloudflare accepts: 1 to 63 lowercase letters, digits or `-`, starting and ending with a letter or digit. (`bynk deploy` suffixes `-<env>` to the name in a non-default environment, which counts toward the 63.)

**Fix:** give the queue a valid name matching the Cloudflare queue you are binding to.

## `bynk.queue.return_not_queue_result`

```text
[bynk.queue.return_not_queue_result] `on message` handler must return `Effect[QueueResult]`, but got `Effect[Result[(), String]]`
```

**Cause:** the handler's return type isn't `Effect[QueueResult]`. A message
handler reports what happened to the message as a `QueueResult` verdict, not a
`Result`.

**Fix:** return `Effect[QueueResult]` — for example `Ack` when the message is
handled, or `Retry("reason")` to have it redelivered. See the
[queue reference](/book/reference/queue/).

## Other queue errors

- `bynk.queue.duplicate_consumer` — two queue handlers in the context consume
  the same queue. Give each a distinct queue name.
- `bynk.parse.handler_in_agent` — `from queue` was placed in an `agent`. Queue
  consumers belong in a `service`.

## Related

- [Process a queued message](/book/guides/entry-points/queue/)
- Reference: [Queue](/book/reference/queue/)
