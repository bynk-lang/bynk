---
level: minor
changelog: `e.detach()` on any `Effect[T]` stops waiting for a running effect and reports a fault or an `Err` through the runtime instead of losing it
probes: diagnostic_coverage
---

## ADR: detach-effect-method
title: `e.detach()`, a compiler-inserted failure route for detached effects
summary: A kernel method on `Effect[T]` that hands a running effect to `waitUntil`, with a runtime-reported failure route

**Context.** `~>` carries two meanings, "do not wait" and "unit only" (#3183). A detached `Err` has no caller to receive it, so splitting them is only coherent if a detached failure is reported. Proposal #3437 (accepted) settles the route and the surface.

**Decision.** `e.detach()` is a kernel method on every `Effect[T]` returning `Effect[()]`, beside `within` and `race`: it resolves as `Callee::Kernel` and joins the needs-context fixed point. It never starts anything (effects are eager promises); it registers the running promise with the execution context's `waitUntil` and completes at once. Failures are reported by the runtime, not by `Logger`: a rejection is passed on unchanged as a rejected promise into `waitUntil` (the `__bynkReportFault` mechanism, #2192), labelled with the detach site; when `T` is a `Result` an `Err` is one structured `console.error` record (`{message, level, fields: {site, type, variant}}`), carrying the error's type name and variant tag and never its payload. The route is chosen from the static type. The test harness's execution-context double records each report and does not fail the case (ADR 0478 stands); the runner prints them under the case and counts them as `detached_failures` in `--format json`. Refused in an agent handler (`bynk.detach.in_agent_handler`), for a store operation there (`bynk.detach.store_op`) and in a pure body (`bynk.detach.in_pure_context`); a function using it called where there is no context is `bynk.exec.unavailable`. Supersedes ADR 0106 D2 and D3 (D1 stands), amends ADR 0478, ADR 0487 and ADR 0693 B. `~>` is untouched here; its deprecation and removal follow in later increments.

**Confirmed on `workerd` (the proposal's two premises).** A rejected promise handed to `waitUntil` is recorded as an exception with the original stack and leaves the response unchanged (`detach_workerd`). Whether Cloudflare applies an uploaded source map to it is the platform's and was not checkable locally. The 30-second post-response `waitUntil` limit is not enforced by local `workerd` (a 40-second task completed) and the platform documentation could not be reached from this environment, so no figure is stated.

**Consequences.** One new kernel method, three diagnostics, one runtime helper (`__bynkDetach`), one harness hook. A program with no `detach` emits as before.
