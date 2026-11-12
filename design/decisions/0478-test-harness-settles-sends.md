# 0478 — The test harness supplies an execution context and settles `~>` sends

- **Status:** Accepted (v0.334). **Amended by [[0824]]** (the exec double records the failures a detached effect reports, and does not fail the case).

**Context.** A handler whose body uses `~>` hands the send's promise to
`deps.__exec.waitUntil`, which the Worker entry threads from the request's
`ExecutionContext`. The test harness supplied no `__exec`, so a case that
called such a route passed `bynkc check` and failed `tsc` (#1994); the system
tier's in-process Worker `fetch` likewise lacked its `ctx` argument.

**Decision.** For a target with a sending handler, `__makeTestDeps` carries an
`__exec` whose `waitUntil` collects promises, and each addressed service call
in a case is wrapped so the collected sends are awaited (and any sends they
start, until none remain) before the call's outcome is bound. A rejected send
does not fail the case, matching `waitUntil` on Workers. The system tier and
integration harness hand a sending participant's `fetch` the same double and
settle it before the response returns. Targets that never send emit unchanged.

**Consequences.** A case observes a send's effects, such as an agent it
updated, in the expectations that follow the call.
