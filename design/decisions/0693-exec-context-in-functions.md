# 0693 — The execution context reaches effectful functions as a hidden trailing parameter, inferred by the compiler

- **Status:** Accepted (v0.475)

**Context.** `~>`, `e.within(d)` and `a.race(b)` hand work to the execution context's
`waitUntil`, which a service handler reaches as `deps.__exec`. A function has no `deps`:
a `~>` in an effectful `fn` checked clean and then failed `tsc` with `TS2304: Cannot
find name 'deps'` (#2682), and ADR 0687 E and ADR 0691 E refused `within` and `race` in
a `fn` for the same reason, telling the author to move the call into the handler. That
inverts the factoring: the helper that knows a call is slow cannot bound it. The context
exists at run time, and every function is called, in the end, from some handler; what
was missing was a path for it. Proposal #2719 supplied the decisions below.

**Decision.**

- **A. A hidden trailing parameter, inferred.** A function in the *needs-context set* is
  emitted with a final `__exec: __BynkExec` parameter, after any comparator witnesses,
  and every call passes the caller's context: `deps.__exec` in a handler, its own
  `__exec` in another function. Static, on every target, and the checker can refuse a
  call from a place with no context at check time, as #2017 does. **Rejected:** an
  ambient context through `AsyncLocalStorage` (Workers need the `nodejs_als` flag, a
  Bundle host may have none, and a function reached from an agent handler would fail
  only at run time); an explicit parameter the author writes (it exposes a platform
  detail and every caller threads it); refusing `~>` in a `fn` with a new code (small,
  but it makes the `within` and `race` refusals permanent).
- **B. The set is a fixed point over the build.** A function is in the set if it is
  effectful and its body, lambdas included, uses `~>`, `within` or `race`, calls a
  function in the set, or names one as a value. A `within` or `race` counts only when
  the checker resolved it as the kernel method on an `Effect` receiver (`Callee::Kernel`
  with a `Ty::Effect` receiver), never by name, so a user type's own `race` or `within`
  method puts nothing in the set. A `~>` is a statement variant and cannot be shadowed.
  Methods count as free functions do; a generic function is in or out as a whole. Units
  are checked dependency-first, so a unit extends the set its dependencies left;
  `check_unit_files` now checks every file of a unit before emitting any, because a
  function's signature depends on callees that may sit in a sibling file. The emitter's
  gates (`deps.__exec` on a handler, the Worker entry and `compose`, the Bundle surface,
  the test module's deps and its virtual-time clock) read the set instead of re-walking
  the syntax, which also stops a user method named `within` from threading an `__exec`
  nobody reads.
- **C. Where a function in the set may be called.** Where an execution context is in
  scope: service handlers (event, cron and queue handlers included), a Bundle `on call`
  handler, lambdas written in any of these, functions in the set, and test `case` bodies
  (G). Anywhere else is `bynk.exec.unavailable`, whose note names the chain down to the
  construct (`` `quoteBoth` → `race` ``) and whose label marks it: an agent handler, a
  WebSocket handler and a provider operation. The direct per-construct codes stay for a
  direct use. A Bundle `on call` handler counts all three constructs, plus calls into the
  set, which closes the known gap where a direct `~>` there emitted `deps.__exec` for a
  `deps` without it.
- **D. A function in the set used as a value** closes over the context where it is named:
  `xs.parTraverse(quoteOne)` lowers to `xs.parTraverse((...a) => quoteOne(...a, __exec))`.
  The value's Bynk type is unchanged. The checker now records every function named as a
  value (`Callee::FnValue`, with no witnesses when the function has no `Ord` bound), so
  the analysis sees the edge; the lowering still builds a closure only for a function
  with witnesses or one in the set.
- **E. The parameter is invisible to Bynk.** The contract guard forwards `__exec`
  unchanged; hover, signature help and generated docs show the Bynk signature. The
  emitted TypeScript signature gains `__exec` last, which is codegen, not ABI (ADR 0310
  D1 limits the published emit ABI), so no published contract changes.
- **F. Output stability.** A function outside the set, its callers and handlers that do
  not reach the set emit byte for byte as before. No existing golden has a `fn` using the
  three constructs, because those failed `tsc` or were refused.
- **G. Test cases.** A `case` body may call an effectful function directly (verified:
  a case body is checked against an `Effect` return type). When it calls a function in
  the set it passes the harness's exec double, and the module goes onto virtual time
  when that function reaches `within` or `race`, so a helper can be unit-tested with
  `stub … after` and no service around it. A direct `~>`, `within` or `race` written in a
  case body stays refused.

**Consequences.**

- `bynk.within.outside_handler` and `bynk.race.outside_handler` no longer fire in a `fn`;
  their notes now describe a provider operation, a WebSocket handler and a test `case`
  body. The WebSocket and provider-operation refusals stand.
- An agent handler that calls a helper using `~>` used to pass the checker and fail
  `tsc`; it is now refused at check time. The changelog lists it.
- `consumes bynk { TimedOut }` is refused (`cross_context_unknown_capability`) although
  the `timed_out_not_in_scope` note recommends it; a unit writes `consumes bynk`. Left
  to a separate fix.
- Hover, completion, semantic tokens and the formatter are unchanged; the diagnostics
  page gains `bynk.exec.unavailable`.
