---
level: patch
changelog: A conformance harness drives a call, a refusal, a fault and an event between contexts through every topology (separate Workers, a Worker group) and asserts the same observations in each (#3562, G3-0, tests only)
---

One project (a caller, a callee, a publisher and a subscriber) is built once per
topology and run under Node with a Service Binding stood in for by the target
Worker's `fetch`. The scenarios assert that a callee's fault reaches its caller as
`CalleeFaulted` and a refused caller gets a bare `403` reported as `Transport`,
that neither a callee's nor a subscriber's own exception reaches a caller, a
publisher or a client, and that an event is delivered, with the same observations
in every topology. G3-2's in-process calls add a topology to this list.
