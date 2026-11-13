---
level: minor
changelog: `Ai.runStream(model, input)` streams a Workers AI model's own events as a `Stream[String]`, ready to serve with `Streaming`; a refusal before the stream is an `Err`, a failure after it faults the stream, and a client that hangs up cancels the model
probes: emit_abi_shapes, ts_writes
---

## ADR: workers-ai-run-stream
title: `Ai.runStream` passes a Workers AI model's streamed events through as text
summary: One element per Server-Sent Event, `[DONE]` consumed, the event shape left to the model

**Context.** #3478 accepted `runStream` as a plain `Stream[String]` whose mid-stream failure faults. Cloudflare documents that `env.AI.run(model, { stream: true })` returns a stream of Server-Sent Events, and that a model's streaming output is binary `text/event-stream`. It does not document the fields of an event, a `[DONE]` terminator, where `usage` appears, or what a failure after the first event looks like.

**Decision.** The binding sets `stream: true` on the caller's input and decodes the SSE body per the SSE format (events end at a blank line; LF, CRLF or CR line ends; `data:` lines join with a line feed; `event:`, `id:`, `retry:` and comments carry no data). Each event's data is one element, as the model sent it; a `data: [DONE]` event ends the stream and is not yielded. Nothing reads the event's fields, so the binding makes no claim about the shape: a handler that wants text deltas maps the stream and decodes each event, and `Streaming(events)` re-frames each as one SSE event. The `response` field commonly seen on chat models' events is **observed, not documented**, and no code depends on it. A refusal before the stream starts is the `Err`; a failure after the first event faults the stream. The decoder is an iterator of its own rather than a generator, so cancelling it (a handler abandoning the stream, a client that hangs up) releases the model's stream at once and never waits behind a read in flight. On workerd a hang-up is learned when the Worker next writes to the client, so a model emitting nothing but comments is not noticed until its next event.

**Consequences.** `runStream` works for any model that streams, whatever its event shape, and a model that does not stream is `AiFailed`. Whether a stream counts against the Worker's wall-time limit as `Fetch` streams do is not verified, and the documentation does not claim it.
