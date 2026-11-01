# 0687 — `e.within(d)` bounds an effect's time, hands the abandoned effect to `waitUntil`, and tests drive it on virtual time

- **Status:** Accepted (v0.470)

**Context.** A handler that fronts a slow dependency had no way to bound its own
latency; the only timeout in the language was `FetchOptions.timeout`, honoured by
`Fetch` alone. The concurrent-effects track (spine #2615) adopted a general timeout
combinator (Q6) and ruled out cancellation (Q7), and its constraint that no work
outlives the handler unseen means a timeout cannot simply drop the promise: on
Workers a promise left running when the response returns is cancelled, possibly
halfway through a write. This record carries the decisions slice 3 (#2683) fixed.

**Decision.**

- **A. Result type.** `Effect[Result[T, TimedOut]]`, with `TimedOut { after: Duration }`
  a new `bynk` record. **Rejected:** `Option[T]`: an `Option?` in an `HttpResult`
  handler becomes `NotFound` (ADR 0177), so a timeout would read as a missing
  resource. `Result` keeps the timeout named and composes with `?` and with
  `embeds TimedOut as V` (ADR 0178); ADR 0178 F deliberately has no automatic lift
  into a status. Naming the result type requires `TimedOut` in scope: a unit that calls `within` must consume `bynk` to name `TimedOut`
  (`bynk.within.timed_out_not_in_scope`, whose note quotes the line to add, `consumes bynk { TimedOut }`), as `Idempotency` and `Events` already need `consumes bynk`; the checker will not import it implicitly.
- **B. Effects that already return a `Result`.** No special case:
  `Effect[Result[T, E]].within(d)` is `Effect[Result[Result[T, E], TimedOut]]`. A
  flattening variant is left until real code asks for it; one method with one rule
  is easier to learn than two.
- **C. The abandoned effect.** `within` races the effect against a timer. If the timer
  wins the call yields `Err(TimedOut { after: d })` and the effect is handed to the
  execution context's `waitUntil` **uncancelled**, so its side effects still happen; a
  fault in it afterwards is the platform's to log. If the effect wins, the timer is
  cleared. A tie goes to the effect. A deadline outside (0, 2^31 - 1] ms faults the
  call (as `FetchOptions.timeout` does), handing the already-running effect on first.
  `FetchOptions.timeout` stays the tool when the work must stop.
- **D. Agent handlers.** Refused (`bynk.within.in_agent_handler`), a lambda in the
  handler included: an agent handler has no execution context, the reason `~>` is
  refused there (#2017). Using the Durable Object's own `state.waitUntil` needs a check
  against workerd for an agent that has replied and an argument that an abandoned
  effect does not break the handler's atomic commit (ADR 0109); it is a follow-up.
- **E. Where the execution context comes from.** Service-handler bodies only. The
  `__exec` gate (`block_uses_send`) widens to a body that calls `within`, so a
  lambda written in the handler closes over its `deps`. An effectful `fn` or a
  provider operation is `bynk.within.outside_handler`; a pure body is
  `bynk.within.in_pure_context`. No function has an execution context today (#2682);
  threading one is that bug's fix, and `within` in a function can be allowed in the
  same change. A WebSocket handler (`on open`, `on message`, `on close`) is refused too, as
  `bynk.within.outside_handler`: it runs in the connection-hosting Durable Object,
  whose `deps` carry no execution context. The Bundle target's `__makeSurface` deps gain `__exec` for a context
  with a `call` handler that needs it, supplied by a composition-root `waitUntil` that
  keeps the promise alive and logs its fault.
- **F. Production timer.** A runtime helper, `__bynkWithin(promise, ms, exec)`, in the
  `settle` module, outside ADR 0310's ABI like `settleAll`. It reads `exec.timers`
  in place of `setTimeout` when present, which is how tests supply virtual time.
- **G. Testing.** Virtual time at `unit` and `integration`. The harness clock gains a
  timer queue; `stub Cap.op(_) returns v after d` completes when the clock reaches `d`
  from the call, and `stub Cap.op(_) never` does not complete (`after` and `never` are
  contextual words; `after` is on `returns` only; `bynk.stub.after_type`). **Guarded
  auto-advance:** when an addressed call has not settled after a macrotask turn, the
  harness fires the earliest queued timer, one at a time with a turn between, only if
  every capability call in flight is a stub waiting on `after` or `never`; in-flight
  calls are counted by wrapping each capability on the test `deps`. A real provider
  holds the clock back; a host timer outside any capability call is not seen and waits
  in real time. Ties go to the effect (effect completions sort before deadlines).
  `advance Clock by d` fires the timers due by then, synchronously. Abandoned effects
  settle at the end of the addressed call, as #1994 does for `~>`; an abandoned `never`
  is left. A call that can never complete and has no `within` faults the case. Only a
  module whose target calls `within` or whose stubs take time takes the new clock;
  every other test module is byte for byte unchanged. At `system` and under
  `--against`, `within` uses real time; `stub` and `advance` are refused there already.
- **H. Observation.** Unchanged. The abandoned call is recorded when issued, so
  `expect A.op before B.op` still compares issue order (track Q5).

**Consequences.**

- Adding `TimedOut` to `bynk` adds its interface and wire codec to the emitted
  `bynk.ts`, and its name to the namespace destructure of every test module that
  imports it. That is the whole change to existing output; every construct that does
  not call `within` or use a timed stub emits as before.
- A timed-out effect may still have happened, so a retry may do the work twice. The
  spec, the reference and a guide say so.
- Auto-advance is as safe as the harness's view: a capability call it cannot see (a
  host promise outside any capability) is not counted, and a case waiting on one
  waits in real time.
- Every addressed call in a test module that has a timed stub or whose target calls
  `within` is driven, so a latency stub is never left waiting for a clock nobody
  moves; the gate for that is syntactic, so a user method named `within` on another
  type would also put its target's test modules on virtual time (harmless: no
  behaviour differs).
- A WebSocket handler (`on open`, `on message`, `on close`) runs in the
  connection-hosting Durable Object, whose `deps` carry no execution context, so it
  is refused with `bynk.within.outside_handler`, as a function is. An event, cron or
  queue handler runs in its Worker entry with `ctx`, and takes `within`.
- Auto-advance counts capability calls and stub waits, not what a provider does
  between them. A provider operation waiting on several stubs at once is driven; one
  waiting on a stub and on real I/O at once is treated as waiting on the stub.
