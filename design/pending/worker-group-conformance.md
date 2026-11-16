---
level: patch
changelog: A conformance harness drives a call, a refusal, a fault and an event between contexts through every topology (separate Workers, a Worker group) and asserts the same observations in each (#3562, G3-0), and a `[deploy.workers]` group's `<member>.bynk-client.ts` is left out of the generated `tsconfig.json` as a context's `bynk-client.ts` is
---

One project (a caller, a callee, a publisher and a subscriber) is built once per
topology and run under Node with a Service Binding stood in for by the target
Worker's `fetch`. The scenarios assert that a callee's fault reaches its caller as
`CalleeFaulted` and a refused caller gets a bare `403` reported as `Transport`,
that neither a callee's nor a subscriber's own exception reaches a caller, a
publisher or a client, and that an event is delivered, with the same observations
in every topology. G3-2's in-process calls add a topology to this list.

The harness found that a group member's typed client, `<member>.bynk-client.ts`,
was not matched by the generated `tsconfig.json`'s `**/bynk-client.ts` exclude, so
a grouped build with a foreign-caller actor failed `tsc` for want of
`@bynk/wire-client`. The exclude now also names `**/*.bynk-client.ts`, and the
harness runs `tsc` against the build's own `tsconfig.json`.
