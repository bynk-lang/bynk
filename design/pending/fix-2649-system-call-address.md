---
level: patch
changelog: A `system`-tier case that calls an `on call` service is refused with `bynk.test.call_needs_in_process`, not lowered to invalid TypeScript (#2649)
probes: diagnostic_coverage, emit_diagnostics
---

## ADR: system-tier-call-address-refused
title: A system-tier case can't call an `on call` service
summary: bynk.test.call_needs_in_process closes the `/* unknown */` hole for `svc.call(…)` at `system`

**Context.** A `system` case drives the target through its real Worker. `http` addresses go through `worker.fetch`, and `cron` and `queue` addresses through `worker.scheduled` / `worker.queue` (#2136). An `on call` service has no such entry: it is internal to a deployment and each call is signed per caller (#2004), so there is no public route for a driver to send to. A case `save.call("abc")` at `system` passed `bynkc check` whenever the target also had an `http` service (so `system_needs_wire` did not fire), and lowering then emitted `/* unknown */.call(save, "abc")`, which `tsc` rejected.

**Decision.** The checker refuses the address at `system` with `bynk.test.call_needs_in_process`, as `bynk.test.event_needs_in_process` does for `svc.event(…)`. The note points to the alternatives: drive an `http` route that reaches the handler, or call the service at `unit` or `integration`. We do not build a system driver for `/_bynk/call/`: it would need a caller identity and a per-pair signature the case has no principal for, and `--against` already refuses an `on call` address for the same reason.

**Consequences.** No `system` case can reach the `/* unknown */` receiver-namespace fallback through a service address. The fallback itself stays, because a program that failed checking is still lowered and its output discarded.
