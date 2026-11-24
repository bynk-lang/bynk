---
level: minor
changelog: A consumed context's service is a `stub` seam at `unit` and `integration`: `stub Pay.authorise(_) returns Ok(…)` substitutes it, `expect Pay.authorise called once` observes it, and `bynk.tier.cross_context_needs_system` names only the services left unstubbed
---

## ADR: stub-consumed-services
title: A consumed context's service is a `stub` seam below `system`
summary: `stub` substitutes a consumed service, so a handler that calls another context is testable in-process

**Context.** Below `system`, no consumed context is stood up, and `stub` doubled
capabilities, not a context's services. A case that reached another context's
service was refused (`bynk.tier.cross_context_needs_system`), so every handler
that calls another context could only be tested by deploying every context it
calls, and a call's failure could not be produced on demand, which left
compensation paths untestable. A consumed service is consumed (`consumes
shop.pay`) and called like a capability, so it should be substitutable like one.

**Decision.**

- **A.** The left side of a `stub` names the service through the consuming file's
  name for the context: `Pay.authorise(<pattern>, …)` with an alias,
  `shop.pay.authorise(…)` without. The argument patterns are a capability's, and
  the right side is typed against the service's declared return type. The
  context is a seam and each stubbed service one operation of it, with the
  parameters and return type of the service's `on call` handler, so the stub is
  checked and lowered as a capability operation is.
- **B.** A stub makes the call legal; an unstubbed call is still refused.
  `bynk.tier.cross_context_needs_system` is reported only for the services a case
  reaches without a stub (its own, or its suite's), naming each. Nothing that
  compiled before changes meaning, and a case cannot run a real cross-context call
  in-process by accident. An unknown service is `bynk.stub.unknown_op`.
- **C.** Deferred. The recommendation was that `integration` round-trip the
  stubbed value and the arguments through the wire codecs (ADR 0009's simulated
  wire). Both tiers pass values directly for now: the value is type-checked
  against the callee's declared return type, but a type that would not survive the
  wire is not yet refused at `integration`. The stub class carries no tier, so
  doing it properly means giving the harness the codecs of the consumed
  context's types; it is recorded as a follow-up.
- **D.** Observation works on services: `expect Pay.authorise called once` and the
  other `called` forms, `with`, `before`, `never called` and `trace`, as for
  capability operations. The subject is the context's alias, or its last path
  segment when it has none.
- **E.** `fails` is a fault, as for capabilities.

**Consequences.** The grammar of a stub target widens from `Cap.op` to a dotted
path whose last segment is the operation. A stubbed context's double sits in the
test `deps` under `surface.<key>`, which the target's handlers call through, and
under `<key>` at the top level, where the recording wrapper reads it. No
production emission changes. `after` on a service stub inherits the existing
virtual-time limitation of `after` at the unit tier.
