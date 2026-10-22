---
level: patch
changelog: "`bynkc test --coverage` anchors a WebSocket-hosted lifecycle method's header, and the agent's dispatch code after it, to the right `.bynk` lines"
---

## ADR: ws-hosted-method-source-map-anchors
title: WebSocket-hosted lifecycle method headers anchor to their own handler line in the emitter's source map
summary: Where a hosted `on open`/`on message`/`on close` method's generated header and the agent's following dispatch code map

**Context.** Agent handlers (#2278), service handlers (#2282) and provider ops (#2284) anchor their generated headers to their own source line. The Durable Object methods that host a `from websocket` service's `on open`/`on message`/`on close` bodies inside an agent had the same gap: each method's header mapped to the previous method's last body line, and the `fetch`/`webSocketMessage` dispatch code after them inherited the last hosted body's final line, so an uncalled last hosted handler could show a covered line (#2280). `cron` and `queue` handlers were checked and already take the per-handler checkpoint, since they are service handlers.

**Decision.** The agent emitter records a checkpoint at each hosted lifecycle method's start, anchored to that handler's span, and one after the last hosted method, anchored to the agent declaration.

**Consequences.** A hosted lifecycle method's header maps to its own handler line and the dispatch code maps to the agent, whichever handlers run.
