---
level: patch
changelog: An agent call no longer forwards the caller's Workers `env` (secrets and vars) in the inter-Durable-Object request body
---

## ADR: inter-do-env-strip
title: An inter-DO call strips `env` from the forwarded deps
summary: The callee rebuilds `env` from its own binding, so the caller's is never serialised

**Context.** `callDurableObjectMethod` serialised the caller's whole `deps`, which on the Workers target can carry the Worker `env`. Secrets and vars are plain strings, so they reached the internal request body, where a tail worker or request log can read them. Bindings serialise to `{}`. The receiving Durable Object rebuilds `env`, its `given` providers and its events fan-out from its own `this.__env`.

**Decision.** `callDurableObjectMethod` drops every `env` key from `deps`, at any depth, before serialising (one `JSON.stringify` replacer). A `given` provider built with the Worker `env` keeps it as an own field (`this.deps = { env, ... }`), so `deps.<cap>.deps.env` would otherwise still carry every secret. The runtime cannot tell a provider from plain data, so it strips by key rather than by dropping capability entries; the callee rebuilds both from its own `env`. Every agent call (handler, service, WebSocket lifecycle body) funnels through it, so the rule lives in that one place. The other runtime senders (`callService`, `deliverEvent`) pass `env` as a separate parameter used only to sign the request, never in the body.

**Consequences.** The callee's `deps` carries no `env` on the wire. This is safe because of the `rpc_rebuilds_env` invariant: any agent whose handlers read `env` (reach a Service Binding or DO namespace, use a `given` provider or emit events) rebuilds it, and its providers, from its own `this.__env`; an agent that does not rebuild never reads `deps.env`. A principal or other deps field literally named `env` would also be dropped; none exists today.
