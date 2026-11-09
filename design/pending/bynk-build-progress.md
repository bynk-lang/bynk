---
level: patch
changelog: "`bynk build`, `bynk dev` and `bynk deploy` print a `Compiling`/`Finished` line per context on a terminal; `--quiet` turns them off and a pipe is unchanged (#3100)"
---

## ADR: bynk-build-progress
title: Per-context build progress
summary: the driver reports contexts through an optional sink; one terminal-gated printer serves build, dev and deploy

**Context.** A successful build printed one summary line, and a `workers` build of a many-context project gave no sign of which context was compiling. #3054 split this out as item 4; #3100 accepted it.

**Decision.** `CompileOptions` gains an optional `ProgressSink` receiving `Started(context)` and `Finished(context)`, fired where the driver already loops over units in dependency order (`run_checks`), for contexts only and only in a build, not a check. A context with errors gets `Started` and no `Finished`. `None` is the previous behaviour, so the LSP, tests and in-memory builds are unchanged and the emitted files are byte-identical. One printer in `bynk-driver::progress` writes cargo-style lines to stderr only when stderr is a terminal and `--quiet` (a global flag) was not given, so captured output is unchanged. `build`, `dev` and `deploy` use it; `check` stays silent on success, `compile` and `tail` do not report. Contexts are reported per context, not per unit, because the loop is already ordered per unit with its context named.

**Consequences.** `dev` reports its first build only. The accepted proposal asked for only the recompiled contexts on a rebuild, but every rebuild re-checks every context, so there is no subset to report, and printing them all on each save would be the noise the proposal meant to avoid. Incremental compilation would revisit this. The sink is called on the building thread in compilation order; a future parallel build must serialise the calls.
