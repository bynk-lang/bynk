---
title: Follow a request across contexts
---
**Goal:** find every log line one request left, in every context it reached, by one
id.

A request that comes in over HTTP, calls `shop.orders`, which calls `shop.payment` and
emits an event, runs in several Workers. Each Worker has its own logs, so without an id
you would match timestamps. Bynk gives each request a **trace id**, passes it along every
call and event, and writes it on the lines it logs.

## What carries the id

- **Every entry point starts a trace.** An HTTP request, a cron run, a queue batch and an
  email each start one. On Cloudflare the id is the one Cloudflare gives the invocation,
  so the entry Worker's own logs and traces match; where there is none, it is random.
  It is 32 lowercase hex digits.
- **A call carries it** in the standard W3C `traceparent` header, with a new span id on each
  hop, so any OpenTelemetry tool reads it too. The callee keeps the caller's trace id.
- **An event carries it** in `EventEnvelope.trace`, an `Option[String]`, so the handler of a
  subscriber, which runs later and in another Worker, is in the same trace.
- **A queue message does not.** A message is only a body, so carrying an id would change the
  message format. A producer that wants one puts an id in the message itself.

A `traceparent` is believed only from a request that has proved who sent it: a signed call
from another context, a signed event delivery, or a signed [foreign
caller](/book/guides/entry-points/http/). A `traceparent` on a public HTTP request is ignored,
and the request gets a fresh trace, because an id a client chooses could be used to forge or
flood correlations.

## What writes it

- `Logger.log` adds `trace` to the record's `fields`, whatever fields you pass. A field of your
  own named `trace` is replaced.
- Bynk's own lines, which are the fault log of an entry, a failed call (`ContractMismatch`,
  `CalleeFaulted`, `CallUnreachable`) and a detached effect that failed, carry it too: a
  line of text ends ` trace=<id>`, and a record has `fields.trace`.
- `Logger.info`, `warn`, `error` and `debug` write the message as it is, so they carry none. Use
  `Logger.log` where you want a line you can follow.

```bynk,fragment
let _ <- Logger.log(Info, "charging", Map.empty().insert("orderId", id))
```

On Cloudflare, search Workers Logs for `fields.trace` with the id, in each context's logs.

## What a handler can read

An event handler that declares the envelope reads the trace as `env.trace`, which is `None` when
the event was emitted outside a trace:

```bynk,fragment
service OnPlaced from Events(Placed) {
	on event(e: Placed, env: EventEnvelope) -> Effect[()] {
		// env.trace: Option[String]
		()
	}
}
```

## When there is no trace

A trace never fails a request. It is held in `AsyncLocalStorage`, which a Cloudflare Worker has
through the `nodejs_als` compatibility flag. Bynk adds that flag to each generated
`wrangler.toml` beside `global_fetch_strictly_public`, and leaves it out where your
[`[workers] compatibility_flags`](/reference/tooling/manifest/) already lists `nodejs_compat`,
which provides it. Where it is missing anyway, a request is served and logs carry no trace.

**See also:** [Reference: the wire format](/reference/wire-format/), [Reference:
`bynk` capabilities](/reference/bynk-capabilities/#log-levels-and-structured-fields),
[Reference: queues](/reference/queue/).
