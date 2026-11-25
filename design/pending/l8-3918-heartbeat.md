---
level: minor
probes: ts_writes
raises: ts_writes — Streaming gains an optional heartbeat interval and the SSE body writes comment lines while idle (#3918)
changelog: Streaming(…).heartbeat(every) keeps an idle event stream open by writing a comment line whenever the stream has produced nothing for the interval
---

## ADR: sse-heartbeat
title: Streaming(…).heartbeat(every) writes comment lines while the event stream is idle
summary: A response method on the Streaming variant, applied when the body is serialised, with resumption left to the handler

**Context.** A long-lived event stream that is idle between events is closed by intermediaries, and a handler could do nothing about it, since every byte on the wire came from an element of its own stream.

**Decision.** (C) `heartbeat(every: Duration)` is a response method of the `#3915` kind, legal only on `Streaming(…)` (written on the variant itself, optionally after other response methods) and refused under one second. It lowers to a `heartbeatMs` field on the result; the runtime's body then races the pending read against a timer and, if the timer wins, writes a `:` comment line and keeps the same read for the next pull, so the source is advanced exactly once per element and no event is dropped, repeated or delayed. It needs no execution context: it runs when the response body is read, not in the handler, so there is no timer to inject (unlike `within`, which runs inside an effect). (D) Resumption is the handler's: Bynk keeps no event history, a handler reads `Last-Event-ID` through a typed `headers` record, as the reference shows.

**Consequences.** A heartbeat holds the Worker invocation open for as long as the connection lasts, so it is opt-in, and the reference says what it does and does not do about platform limits. `heartbeat` appears in completion and signature help for an `HttpResult` receiver like the other response methods, and a non-`Streaming` receiver is `bynk.types.method_not_found`. A source that throws while a heartbeat is armed still errors the body, in-band, as it does without one.
