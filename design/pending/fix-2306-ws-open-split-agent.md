---
level: patch
changelog: "A WebSocket `on open` that transfers its connection to an agent in another file of a split context is now a compile error on the Workers target (`bynk.ws.open_transfer_cross_file`), not a silent `404`"
---

## ADR: ws-open-transfer-same-file
title: A Workers WebSocket `on open` and the agent it transfers to must share a file
summary: Why a cross-file transfer is rejected on Workers, and why it is not supported

**Context.** On Workers, a `from websocket` service's `on open` runs inside the Durable Object of the agent it transfers the connection to: the emitter finds the services that transfer to an agent among the items of the file being emitted, and writes the `__wsOpen_*` method and a `/_bynk/ws/open/<service>` branch into that agent's class. A context split across several files is emitted one file at a time, so when the service and the agent were in different files nothing hosted the open. `compose.ts` still forwarded the upgrade to the agent's Durable Object, whose `fetch` had no such branch, so the upgrade returned `404`. The check for a routable transfer (`bynk.ws.open_transfer_shape`) is unit-wide and the transfer target is a unit-wide name, so the layout compiled (#2306).

**Decision.** On the Workers target, a transfer into an agent declared in a different file than the service is a compile error, `bynk.ws.open_transfer_cross_file`, raised from a Workers-only project phase beside the other target-gated checks. It is not supported: hosting it would mean lowering the service's handler body into the agent's file with the service file's types and bindings imported, which is a larger design than this fix. The bundle target is unaffected: there the open is an ordinary surface method and the layout works.

**Consequences.** The failure moves from a runtime `404` to a compile error with a note saying to move the service into the agent's file. Supporting the cross-file transfer remains possible later, by resolving the transferring service across the whole unit when emitting the agent.
