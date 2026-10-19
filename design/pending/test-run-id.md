---
level: minor
probes: ts_writes, emit_diagnostics, diagnostic_coverage
changelog: A test case reads `run.id`, the run's identity — `"local"` in process, fresh for each `bynk test --against` run — so data a case creates on a shared deployment doesn't collide with an earlier run's. Under `--against`, an `expect` comparing an `Instant` built from a constant warns (`bynk.test.remote_clock_dependent`). New guide, *Testing against a running system* (#2138)
---

## ADR: test-run-identity
title: A case reads the test run's identity as `run.id`
summary: `run.id` is a `String`, `"local"` in process and fresh per remote run; `run` is contextual, shadowed by any binding

**Context.** End-to-end testing track §3.8 (#2138). A real deployment keeps its state between runs, so a case that creates `order-1` finds it there the second time, and two runs against one preview deployment collide. The in-process harness starts every case fresh, so it never needed a name for the run.

**Decision.**

- **`run.id`**, a `String`, read in a `case` body. In process it is always `"local"`, so the in-process run stays deterministic. Each `bynk test --against` run sets a fresh value (the time and the runner's process id); every `bynk dev` session of one `--against dev` run shares it. The runner passes it to the harness as `BYNK_TEST_RUN_ID`.
- **`run` is contextual, not a keyword.** It names the test run only where nothing else binds `run`: a `let run = …` shadows it, and a history property's `for all run: History[…]` is an ordinary binding. So no existing program changes meaning, and `run` stays usable as a name.
- **Tooling.** Hover on `run.id` in a case body describes it, and completion after `run.` offers `id` there (ADR 0156).

**Alternatives.** A `Val[RunId]` fabrication was rejected: a fabricated value is random per case, but a case needs one value shared by the run (and by its `bynk dev` groups) so a later run can tell its data apart. A reserved keyword was rejected because `run` is already a common binding name.

**Consequences.** A case that keys its data by `run.id` reads the same in process every time, and on a deployment leaves one set of data per run behind; the runner doesn't clean it up.

## ADR: remote-clock-dependent-warning
title: Under `--against`, an `expect` on an absolute `Instant` is a warning, not a refusal
summary: `bynk.test.remote_clock_dependent` warns when a remote `system` case compares an `Instant` not derived from the run

**Context.** Track §3.4 and §3.7 (#2138). In process the harness clock starts at the epoch and every participant reads it (ADR for #2137). A real runtime's Workers read wall-clock time, which the runner can't reach, so `advance` is refused remotely. A case can still compare an instant against a constant (`Instant.fromEpochMillis(0)`), which holds in process and fails against a real runtime, and no `--settle` retry helps.

**Decision.** A remote build warns, once per case at the first such `expect`, when a comparison's `Instant` operand isn't run-derived. An instant is run-derived when it comes from a binding the case got from an address (or a field of one, or one shifted by a `Duration`); one built from a constant, or a `let` bound to one, isn't. It is a warning because whether an `expect` really depends on the clock can't be decided exactly, and a refusal would block cases that are correct.

**Consequences.** In process nothing changes. The documented remedy is to compare instants the run produced (`expires > created`) rather than absolute ones.
