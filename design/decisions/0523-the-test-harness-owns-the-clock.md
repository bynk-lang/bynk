# 0523 — The test harness owns the `Clock` at every tier, and `advance Clock by` moves it

- **Status:** Accepted (v0.363)

**Context.** At `unit` and `integration` the `Clock` double reads the epoch (ADR 0431). At `system` a participant's `Clock` was `Date.now()`, and `stub Clock.now` is refused there (`bynk.stub.at_system`). So a promoted case that depended on time changed meaning and became non-deterministic at `system`. A case also couldn't say "fifteen minutes pass" between two calls. The end-to-end testing track (§3.4, #2137) settles whether the harness may own time at `system`.

**Decision.**

- **A — not a `stub`.** A `stub` overrides one operation of one context's seam, layered over a default. The harness clock is the single time source every participant reads, installed in the environment the harness gives each Worker, as ADR 0467's in-memory KV and Durable Object namespaces are. That is why it is legal at `system` while `stub Clock.now` stays refused there. At `unit` and `integration`, a `stub Clock.now` still wins for the operations it names.
- **B — it starts at the epoch**, as the lower tiers' double does, so a case reads the same `Clock.now` at every tier. Each case starts its own.
- **C — it governs the first-party `Clock` only.** The runtime's own clocks (JWT `exp`, the `Signature` tolerance, Durable Object alarms) keep wall-clock time, so a test token never expires because a case advanced the clock. A context's own `provides Clock` is untouched.
- **D — `advance` is legal at every tier.** At `unit` and `integration` it moves the `Clock` double, which ADR 0431 now describes as "the epoch plus what the case has advanced".
- **E — the spelling is `advance Clock by <Duration>`.** It names the capability it governs. `advance` is contextual, opening a statement only before `<Ident> by`. `wait` and `sleep` were rejected because they suggest real time passes. The checker refuses it outside a `case` body (`bynk.test.advance_outside_case`), on any capability but `Clock` (`bynk.test.advance_not_clock`), and with an amount that isn't a `Duration` (`bynk.test.advance_not_duration`). A `Duration` literal can't be negated, so a negative amount is always computed, and it faults the case at run time.
- **The seam is test-build-only.** In the test build of a context a `system` harness stands up, the first-party `Clock` is built as `env.__bynkClock ?? new ClockProvider()`, in `compose` and in an agent's deps alike, and the harness sets `__bynkClock` on every participant's `env`. A deployable build carries no suites, so its output is byte-identical.
- **The scheduled time isn't defaulted.** A cron address keeps its explicit scheduled time (track §3.3). The issue's example defaulted it to the clock's now; the settled track text doesn't.

**Consequences.** A `system` case that reads `Clock.now` reads the epoch, not wall-clock time, so a time-dependent body passes identically at every tier. The clock does not reach a remote run (track §3.4–§3.7, slice 5).
