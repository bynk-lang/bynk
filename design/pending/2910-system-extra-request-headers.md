---
level: minor
probes: emit_diagnostics, ts_writes, diagnostic_coverage
changelog: A `system` case can send request headers to an http route that declares no `headers:` parameter, by a trailing `RequestHeaders` argument on the address
---

## ADR: system-extra-request-headers
title: A system case sends extra request headers by a trailing RequestHeaders argument
summary: Request headers reach a route with no `headers:` parameter only at system tier, as a trailing address argument

**Context.** A `system` case drives a real request, but a `RequestHeaders` value was accepted only in the position of a route's declared `headers: RequestHeaders` parameter. A route that negotiates `Locale.current()` against `Accept-Language` and never reads headers itself could not be tested without adding an unused parameter (#2910).

**Decision.** An http service-address call accepts an optional trailing `RequestHeaders` argument after the route's own arguments: `api.GET("/hello", RequestHeaders.of(["Accept-Language: fr"])) by Visitor`. No new syntax is added (a `with headers` clause was rejected for that reason). A route that declares `headers:` keeps its behaviour: the argument fills the declared parameter, and a second `RequestHeaders` is `bynk.test.service_call_arity` with a note saying why. The form is `system`-only. Below `system` the route sees only its declared parameters, so the case keeps the arity error, with a note naming the tier. The system driver spreads the headers into the request ahead of its own, so a header the driver sets itself (`content-type`, the actor's credential) wins. `Authorization` is refused at check time (`bynk.test.extra_headers_authorization`) rather than silently overridden, because `by <Actor>` owns the credential; the refusal inspects the literal `RequestHeaders.of([...])` form only.

**Consequences.** Each system route without `headers:` gains a required `__extraHeaders` driver slot, which the call lowers to `undefined` when nothing is sent, so every system golden moves. A header built outside the literal form is not inspected for `Authorization`; the driver's own credential still wins when the actor sets one, but a `Visitor` case could still send one that way. Revisit if that proves a hole.
