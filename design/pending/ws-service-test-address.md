---
level: minor
probes: emit_diagnostics, ts_writes, ts_any, diagnostic_coverage
changelog: A case drives a `from websocket` service by address (`svc.open(tc, …)`, `svc.message(tc, frame)`, `svc.close(tc)`) against a recording `TestConnection[F]()`
---

## ADR: test-websocket-service-address
title: A case drives a WebSocket service by address against a recording TestConnection
summary: open/message/close addresses, a case-scoped TestConnection[F], and the principal given once at open

**Context.** A case addressed a service by its natural surface (`svc.call`, an
HTTP verb, `svc.schedule`, `svc.message(m)`, `svc.event(e)`), but a `from
websocket` service had none: `ChatGateway.open(...)` was refused with
`bynk.test.service_bad_address`, and a `Connection` has no constructor, so an
agent that takes an owned `Connection` could not be reached from a case either.
The runtime's capture-and-inspect `TestConnection` existed only for the
compiler's own Rust-driven Node harness. Issue #3051.

**Decision.**

- **Addresses.** `svc.open(tc, route…) by <Actor>(…)`, `svc.message(tc, frame)`
  and `svc.close(tc)`, named for the handlers. `open` takes the case's
  `TestConnection` and the handler's route parameters; `message` takes the
  decoded inbound frame (typed against the service's `in` type); `close` takes
  nothing more. The connection remembers the route values and the principal
  from `open`, as the platform recovers them from the socket attachment
  (#3046), so a case cannot re-supply values production never sees. A
  `message` or `close` on a connection that was never opened fails the case.
  `svc.message` is now protocol-aware: on a `from websocket` service it is the
  WebSocket address, on a queue service it is unchanged.
- **The test connection.** `TestConnection[F]()` is a case-scoped, non-held
  handle (`Ty::TestConnection`). `.sent()` is the frames the server wrote
  (`List[F]`), `.closed()` is whether the server closed it, `.connection()`
  mints the owned `Connection[F]` an agent receives, once per handle. It is a
  separate handle because a `Connection` is linearly owned and cannot be read
  after it is passed on. Only a test body can name it; elsewhere the resolver
  reports an unknown function, so a program still has no way to make a
  connection.
- **Delivery and close.** `message` delivers one already-decoded frame; there
  is no raw-text form. `close` models the client leaving: it runs `on close`
  and does not set `.closed()`, which is true only when the server closed the
  connection.
- **Principal.** `by` goes on `open` only, typed as on any other address.
  `by` on `message` or `close` is `bynk.test.ws_lifecycle_by`.
- **Tiers.** `unit` and `integration`, which run the same in-process module. A
  `system` case is refused with `bynk.test.ws_needs_in_process`: a deployed
  WebSocket is a real upgrade through the Worker and a Durable Object, which
  the system harness does not drive. (A target with only WebSocket services has
  no serialisation edge, so `bynk.tier.system_needs_wire` already refuses
  `system` there.)
- **Delivery reuses the emitted handlers.** The addresses call the service's
  generated `open`/`message`/`close` methods with the `TestConnection` as the
  `connection`, so nothing about the handler is re-implemented.

**Consequences.** A WebSocket service, and an agent that holds connections, are
testable with `bynk test`. Two diagnostics are new: `bynk.test.ws_needs_in_process`
and `bynk.test.ws_lifecycle_by`. The test module imports the runtime's
`TestConnection` only when a body builds one, so existing modules are unchanged.
The case-body linearity pass does not track a minted `Connection`, so an unused
`tc.connection()` is not reported as a leak. A Workers-realised test that
upgrades through the real entry point depends on the workerd harness work in
#2966 and is left for a follow-up. The "no tests passes silently" note in the
issue is a separate runner-reporting change and is not part of this increment.
