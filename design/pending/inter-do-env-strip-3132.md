---
level: patch
changelog: An agent call no longer forwards the caller's Workers `env` (secrets and vars) in the inter-Durable-Object request body
---

## ADR: inter-do-env-strip
title: An inter-DO call strips `env` from the forwarded deps
summary: The callee rebuilds `env` from its own binding, so the caller's is never serialised

**Context.** `callDurableObjectMethod` serialised the caller's whole `deps`, which on the Workers target can carry the Worker `env`. Secrets and vars are plain strings, so they reached the internal request body, where a tail worker or request log can read them. Bindings serialise to `{}`. The receiving Durable Object rebuilds `env`, its `given` providers and its events fan-out from its own `this.__env`.

**Decision.** `callDurableObjectMethod` drops the `env` field from `deps` before serialising. Every agent call (handler, service, WebSocket lifecycle body) funnels through it, so the rule lives in that one place. The other runtime senders (`callService`, `deliverEvent`) pass `env` as a separate parameter used only to sign the request, never in the body.

**Consequences.** The callee's `deps` no longer carries `env` on the wire; a body that reaches `deps.env` already gets it rebuilt from the DO's own `env`.
