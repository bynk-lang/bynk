# Concurrent effect composition — joining different effects, timeouts and races

- **Status:** Settled (#2635, merged 2026-11-01). Every question in §5 is
  closed; the maintainer confirmed the adopted answers on 2026-11-01. Q1 adopted
  the concurrent binding block, so the language gains no tuple type. Slicing:
  slice 0 shipped in #2646 (ADR 0672), with slice 2 folded into it; slice 0b
  shipped in #2669 ([#2662](https://github.com/bynk-lang/bynk/issues/2662));
  slice 1 was dropped (Q2); slice 3, the timeout, shipped in #2693
  ([#2683](https://github.com/bynk-lang/bynk/issues/2683), ADR 0687); slice 4,
  the race `a.race(b)`, shipped in #2713
  ([#2711](https://github.com/bynk-lang/bynk/issues/2711), ADR 0691) and reopened
  Q7 (see its **[Amended]** answer). Every planned slice has shipped. Spine
  [#2615](https://github.com/bynk-lang/bynk/issues/2615).
- **Realises:** the effect model of `design/bynk-design-notes.md` (an `Effect` is
  the only way a program reaches the outside world), extending the
  effectful-iteration matrix of ADRs 0170–0174 from *one effect per element of a
  collection* to *several different effects at once*.
- **Posture:** Feature track per [ADR 0076](../decisions/0076-feature-track-posture.md).
  It qualifies on all three axes (§2): it spans several increments, its surface
  is not settled, and it touches the handler-atomicity guarantees that agent
  state depends on.
- **Governing records:** ADR 0031 (`Effect` is an eager `Promise`, non-storable,
  confined to effectful contexts), 0106 (`~>` send, lowered to `waitUntil`), 0109
  (the handler is the atomic unit for storage; what happens to each effect class
  on a fault), 0135 (`Query.parTraverse`), 0170–0174 (the effectful-iteration
  matrix), 0426 (an unawaited `Effect` value is an error), 0444 (a fault is
  claimed, not caught).

## 1. The theme

Every `<-` waits. These two lines run one after the other, so the handler pays
the sum of the two latencies:

```bynk
let user <- Users.get(id)
let orders <- Orders.forUser(id)
```

They are independent, and on Workers most handler time is spent waiting on
subrequests, agent calls and storage. Issuing both and waiting for both would
cost the larger of the two latencies instead of their sum. Today Bynk can only do
that over a **collection of the same effect**: `parTraverse`, `parTraverseAll`
and `parTraverseTry` on a `List`, `Map` or `Query` (ADRs 0171–0174), each
lowering to `Promise.all(xs.map(f))`. Two *different* calls cannot be joined.

There is also no general **timeout** or **race**. The only timeout in the
language is `FetchOptions.timeout` on `Fetch.sendWith` (standard-library K5a).

### 1.1 What this track settles

1. **The surface for a concurrent join** of different effects, including what
   carries the results (§5 Q1). Bynk has no tuple types and no anonymous
   records, so this is a type-system question as much as a syntax one.
2. **What a join means when something fails**, for `Result` errors and for
   faults (Q2, Q3).
3. **How joins interact with agents**: the staged write-set, cross-agent calls
   and Durable Object gates (Q4).
4. **What tests observe** when operations overlap (Q5).
5. **Timeouts, races and cancellation**, and what Bynk promises about the losing
   side (Q6, Q7).

**End state when this track retires:**

- a handler can issue several independent effects and wait for all of them, in
  one readable form, with deterministic error and fault reporting;
- a general timeout exists, with its semantics for the abandoned work stated;
- agent handlers, `store` writes and test observations behave predictably under
  concurrency;
- sequential code emits exactly as it does today.

## 2. Why a track (the ADR 0076 trigger)

- **Multi-increment.** The join itself, then fallible joins, then timeouts and
  races, then agent and test rules where they do not fold into earlier slices.
- **Surface not settled.** Tuples, a record-shaped join, or a statement form are
  all live options (Q1), and the cancellation stance (Q7) decides how much of
  the capability layer has to change.
- **Safety boundary.** Concurrency inside a handler interacts with the atomic
  commit of ADR 0109 and with Workers' request lifecycle. A wrong shape can
  leave effects running after the handler has replied, or report a different
  error depending on which subrequest finished first.

## 3. What's already true

- **Effects are eager.** "`Effect` is an eager `Promise`: an un-bound effectful
  call is already running" (ADR 0031). `<-` lowers to `await`.
- **An unawaited effect is an error.** ADR 0426 rejects an `Effect` value in a
  plain `let`, a list element, or a constructor payload
  (`bynk.effect.unbound_effect`). That rules out the JavaScript idiom of starting
  two promises and awaiting both later. It is the reason a join needs its own
  surface rather than falling out of existing syntax.
- **Collection fan-out exists.** `parTraverse`, `parTraverseAll` and
  `parTraverseTry` issue every element's effect and lower to
  `globalThis.Promise.all(…)` (`bynk-emit/src/emitter/lower.rs`). So a **fault**
  surfaces at the *first* rejection, and the siblings keep running unobserved:
  "each runs to completion" (§5.10) means they are not cancelled, not that the
  fan-out waits for them. For **`Result` errors** the picture is different.
  `parTraverseAll` gathers every `Result` in input order, and `parTraverseTry`
  awaits every element and *then* returns the first `Err` in input order. Its
  name suggests it short-circuits, but it does not stop waiting early. That is
  the precedent for Q2's adopted answer.
- **`~>` is fire-and-forget.** It lowers to `waitUntil(<effect>)`, is not
  awaited, and is not retracted by a later fault (ADR 0106, ADR 0109 D4).
- **Handler atomicity is per effect class** (ADR 0109 D4). `store` writes are
  staged and flushed once at handler end, so a fault commits nothing.
  Cross-agent `<-` calls commit remotely as each returns and stand after a fault.
  `~>` sends stand.
- **`Effect` combinators.** `map`/`flatMap` on any `Effect`, and
  `mapOk`/`mapErr`/`flatMapOk`/`flatMapErr` on `Effect[Result[T, E]]` (§5.2).
  None of them joins two effects.
- **Test observation** records each capability call in a per-case log; `expect
  A.op before B.op` compares the first recorded order of the two operations
  (§7.4.11).
- **Time.** `Clock` has only `now()`. There is no sleep. Test cases move a
  harness clock with `advance Clock by d` (#2137).

## 4. Constraints on any answer

- **Cost visible at the call site.** A reader must be able to tell at a glance
  which effects overlap and where the wait happens.
- **No unawaited effects.** Whatever the surface is, it must not reopen the
  hole ADR 0426 closed: every effect must be awaited at a point the checker can
  see.
- **Deterministic results.** The value a join produces, the error it reports and
  the fault it raises must not depend on which subrequest finished first.
- **No work outlives the handler unseen.** On Workers, a subrequest still
  running when the response is returned is cancelled unless it was handed to
  `waitUntil`. A join must finish everything it awaits. The only work allowed to
  outlive it is what a construct explicitly hands to `waitUntil` at a visible
  call site: `~>` today, and a timeout's abandoned effect (Q6).
- **Sequential code is unchanged**, byte for byte.

## 5. Open design questions

Each question has options, prior art, and the **adopted** answer. Where an
answer leaves a fact to verify against the code or the platform, the proposal
for the slice that depends on it must do so before that slice is accepted.

### Q1 — What does a join look like, and what carries the results?

- **(a) Tuples.** Add tuple types and an `Effect.both(a, b) -> Effect[(A, B)]` /
  `Effect.all(…)` family, destructured with a tuple pattern:
  `let (user, orders) <- Effect.both(Users.get(id), Orders.forUser(id))`. This
  is the most general option, and the familiar one from Scala, Rust and
  TypeScript. It adds a structural product type to a language whose products
  are all nominal records, and needs tuple patterns (patterns track #2616,
  slice 5).
- **(b) A record-shaped join.** `Effect.all { user: Users.get(id), orders:
  Orders.forUser(id) }`, yielding a record. Bynk records are nominal and
  declared, so this either needs a declared result type at every join site or
  introduces anonymous records, which are a larger change than tuples.
- **(c) A concurrent binding block.** A statement that groups `<-` bindings and
  binds each name in the enclosing scope once all have finished:

  ```bynk
  par {
    let user <- Users.get(id)
    let orders <- Orders.forUser(id)
  }
  // user and orders are in scope here
  ```

  No new type is needed, each binding reads exactly as it does today, and the
  block makes the overlap and the single wait point visible.

**Adopted: (c).** It is the only option that adds no type-system
feature, keeps each call written exactly as a sequential `<-` would be, and
puts the wait at a visible closing brace. Rules for the block:

- it contains only `let x <- e` bindings, `let pattern <- e else …` is excluded,
  and `do e` lines are allowed for unit effects;
- no binding may mention a name that another binding in the same block binds,
  **even where that name would resolve to an outer binding**
  (`bynk.par.sibling_name`). Because names bound inside are only in scope after
  the block, a narrower rule ("may not *depend on* a sibling") would be redundant,
  and would let `let user <- …; par { let user <- A(); let o <- B(user) }`
  silently pass the stale outer `user` to `B`. Rejecting the name outright
  catches that case, and keeps the semantics independent of order;
- it is legal only in an effectful context, like any `<-`;
- names bound inside are in scope after the block, not inside it.

The keyword (`par`, `concurrently`, `together`) is chosen in slice 0's
proposal; `par` is the working name. If the patterns track
later adopts tuples for other reasons, (a) can be added as a library-level
convenience over the same runtime helper, but the track should not depend on it.

### Q2 — `Result` errors and `?` inside a join

A binding may produce a `Result` (`let user <- Users.get(id)` where `get`
returns `Effect[Result[User, NotFound]]`), and may apply `?`.

- **(a) `?` short-circuits on the first `Err` to arrive.** That makes the error a
  handler returns depend on timing.
- **(b) Wait for every binding, then apply each `?` in source order.** The first
  `Err` *in source order* propagates, whichever finished first.
- **(c) Forbid `?` inside a join.** The author matches afterwards.

**Adopted: (b).** It follows the determinism constraint (§4) and
matches how `parTraverseAll` reports outcomes in input order. The cost is that a
fast `Err` waits for a slow sibling; that is the price of the determinism and of
§4's "no work outlives the handler" rule (Q3).

**Revisited after slice 0 (2026-11-01): slice 1 dropped.** The question
assumed a binding could apply `?` to its own effect. It cannot: `?` takes a
`Result` value, never an `Effect`, so `let user <- Users.get(id)?` is
`bynk.types.question_on_non_result` sequentially as well. A binding that
yields a `Result` is unwrapped after the block, and (b) then holds with no
language change:

```bynk
par {
  let userR <- Users.get(id)
  let ordersR <- Orders.forUser(id)
}
let user = userR?
let orders = ordersR?
```

The block has waited for both bindings, and the `?` lines run in source order,
so the first `Err` in source order propagates. The only `?` the block refuses
(`bynk.par.question_in_join`) is one on a pure sub-expression of a binding, such
as an argument, which can be computed before the block. Allowing it would mean
routing the early return out of the binding's thunk and past the join, for a
case the hoist already covers. The refusal is therefore permanent. Its
diagnostic shows both patterns. A shorthand that binds and unwraps an
`Effect[Result]` in one step would be new surface for sequential code too, so it
belongs to its own proposal, not to this track.

### Q3 — Faults inside a join

`Promise.all` rejects as soon as one promise rejects and leaves the rest running
unobserved.

- **(a) `Promise.all` semantics.** The first fault to arrive propagates
  immediately; siblings keep running, and may be cancelled by the platform when
  the response is returned.
- **(b) `allSettled` semantics.** Wait for every binding to settle, then raise
  the first fault in source order (a later fault is logged with it). Nothing
  started by the join is still running when the join completes.

**Adopted: (b).** It is deterministic, and it keeps every effect
the join awaits inside the handler's lifetime (§4). ADR 0109 D4 is unchanged:
cross-agent calls that completed before the fault still stand, and `store`
writes are still staged and discarded.

This **differs from today's `parTraverse` family**, which is option (a) (§3), so
it also breaks §4 today. Two forms with different fault semantics for the same
idea would be a trap. **Adopted:** move `parTraverse`, `parTraverseAll` and
`parTraverseTry` (on `List`, `Map` and `Query`) to settle-all as well, in their
own slice (§6, slice 0b). The change is visible only when an element faults:
the fan-out then waits for its siblings before raising, and raises the first
fault in input order rather than the first to arrive.

### Q4 — Joins in agent handlers

- **Store writes.** `:=` and `.update` are statements, not effects, and stage
  into the handler's overlay (ADR 0109 D2). They are excluded from a join block
  by Q1's rules. Reads inside a binding's expression see the overlay as it was
  when the block began.
- **Cross-agent calls.** Two `<-` calls to *different* agents can overlap
  safely; each commits remotely as it returns (ADR 0109 D4). Two calls to the
  *same* agent instance are serialised by its Durable Object, so a join gains
  nothing but loses nothing either.
- **Durable Object gates.** Awaiting a subrequest already opens the input gate
  in sequential code, so a join does not add any interleaving that sequential
  awaits do not already allow. There is one difference. Sequential code has a
  synchronous gap between line 1 returning and line 2 being issued, in which no
  other request can run, and code can rely on that gap (read a value, then decide
  what to issue next). A join removes that gap while keeping the overall
  gate-open period. Q1's sibling-name rule already stops one binding from using
  another's result, so nothing *inside* a join can rely on the gap. Slice 0's
  proposal must confirm the rest against current Durable Object input-gate
  behaviour, including for `store` reads.
- **Adopted:** joins are legal in agent handlers with the rules above. Calling the
  agent's own handlers from inside a join (re-entrancy) stays exactly as legal or
  illegal as it is sequentially.

### Q5 — What tests observe

The observation recorder logs a capability call when it is *made*. In a join,
calls are issued in source order, synchronously, before any is awaited, so the
issue order is deterministic even though completion order is not.

- That holds only for the **first** capability call each binding makes. When a
  binding's expression is a composite effect (`X.a().flatMap(…)`, `mapOk`, or a
  user function that makes several calls), its later calls are issued when the
  earlier ones complete, so their order against another binding's calls follows
  completion and microtask order, not source order.
- **Adopted:** keep recording at issue time and document that `expect A.op before
  B.op` compares **issue** order. Between the *first* calls of two bindings it
  is source order; for any later call it is whatever order the calls were
  actually made in. Neither is evidence that `A` finished first. Restricting each
  binding to a single direct capability call would make the stronger guarantee
  hold, but would rule out ordinary helper functions, so it is not
  adopted. A test that needs completion order is testing a race and should
  be written against a stub that controls it.
- Stubs resolve deterministically in a test, so a join's result in a test does
  not depend on timing. Combined with Q2 and Q3, a join is reproducible under
  test.

### Q6 — Timeouts

- **(a) Per-capability options only**, like `FetchOptions.timeout`. Each
  capability grows its own option.
- **(b) A general combinator.** `e.within(d) -> Effect[Option[T]]` (or a
  `Result[T, TimedOut]`), applicable to any effect, at a visible call site.

A general timeout cannot stop the work it abandons (Q7). If `e` is a
cross-agent call, the remote handler still runs and commits.

- **Adopted:** (b), shipped after the join, with the abandoned effect handed to
  `waitUntil` so it is allowed to finish rather than being cut off mid-write when
  the response returns. This is the one **deliberate exception** to Q3's "nothing
  the join started is still running": in `par { let x <- e.within(d) }`, the join
  waits for `within`, and `within` detaches `e` explicitly. Q3's guarantee covers
  what a join awaits, and §4 and §8 are worded to allow this exception. The spec must say plainly that a timed-out effect may
  still complete and commit. Per-capability timeouts that *can* abort (`Fetch`,
  through `AbortSignal`) remain the right tool when the work must actually stop.
- Testing a timeout needs the harness clock (`advance Clock by d`) to drive it,
  so the combinator must read time through `Clock`, not the host timer, in a
  test build.

### Q7 — Races and cancellation

JavaScript has no general promise cancellation. Workers cancel outstanding
subrequests when a response is returned without `waitUntil`, and `fetch` can
take an `AbortSignal`. Nothing else in Bynk's capability layer can be aborted.

- **(a) No general race.** Offer only timeouts (Q6).
- **(b) A race whose loser runs to completion** under `waitUntil`, with its
  result discarded. Honest, but every losing side's side effects still happen.
- **(c) Real cancellation**: thread an abort signal through every capability and
  binding so a loser can be stopped. A large change across the stdlib and every
  adapter, and only some platform operations can honour it.

**Adopted: (a) for this track, with (b) named as a possible later
slice and (c) a non-goal.** The motivating case is latency for independent work
(Q1); racing alternatives is rarer, and without cancellation it hides side
effects behind a construct that reads as if the loser did nothing.

**[Amended]** Slice 4
([#2711](https://github.com/bynk-lang/bynk/issues/2711)) adopts (b), reopening
the answer above. Slice 3 had by then built what (b) needs: a method table on
every `Effect`, the `__exec` gate for service handlers, the `waitUntil` handover
for abandoned work, and virtual time with latency stubs. The same stance that
answered this objection for `within` answers it for a race: the loser is handed
to `waitUntil` and runs to completion, the docs say so plainly, and `race` is
for idempotent work such as a hedged read. `a.race(b)` takes the first effect to
*succeed*; if every side faults, the first fault in source order is raised with
the rest as `suppressed`. (c), real cancellation, stays a non-goal, and
`FetchOptions.timeout` stays the tool when work must stop.

## 6. Candidate slice decomposition

Each becomes an increment proposal opened as
a sub-issue of #2615.

- **Slice 0 — The concurrent binding block.** Grammar, the independence check,
  scoping, emission to a settle-all helper, Q3's fault semantics, and Q5's
  documented observation semantics. Value and unit bindings only; no `?`.
- **Slice 0b — Settle-all for the `parTraverse` family**, aligning its fault
  semantics with the join's (Q3). A behaviour change only when an element
  faults.
- **Slice 1 — `Result` and `?` in a join**, with Q2's source-order rule.
  **Dropped** (Q2): the block already delivers the rule with no language
  change.
- **Slice 2 — Agent handlers**, only if slice 0's proposal finds rules beyond
  Q4's adopted answer that need their own increment. Otherwise folded into
  slice 0.
- **Slice 3 — A general timeout**, `within(d)`, with harness-clock support and
  the `waitUntil` handover for abandoned work. Shipped in #2693
  ([#2683](https://github.com/bynk-lang/bynk/issues/2683), ADR 0687). `within`
  is legal only in a service handler's body: it is refused in an agent handler,
  a WebSocket handler, a provider operation and any `fn`, none of which has an
  execution context to hand the abandoned work to. The `fn` refusal can lift once
  [#2682](https://github.com/bynk-lang/bynk/issues/2682) (`~>` in a function)
  is resolved.
- **Slice 4 — Race (loser runs to completion)**, shipped in #2713
  ([#2711](https://github.com/bynk-lang/bynk/issues/2711), ADR 0691). Q7 first adopted no
  general race and named (b) as a possible later slice; slice 4 adopts (b) (Q7,
  **[Amended]**). `a.race(b)` is a method of every `Effect[T]`, legal exactly
  where `within` is, lowered to `__bynkRace`; the loser is handed to `waitUntil`
  uncancelled. (c) stays a non-goal.

## 7. Front-loaded ADR candidates

To land with, or just before, slice 0:

- **The join surface and its result carrier** (Q1). Choosing tuples here would
  add a Foundations-level type; choosing the block form avoids it.
- **Settle-all fault semantics and source-order reporting** (Q2, Q3). They fix
  what every join, and every later timeout, means on failure, and they amend the
  `parTraverse` family's semantics (ADRs 0171–0174) for slice 0b.
- **The cancellation stance** (Q7). It decides whether capabilities and bindings
  must ever carry an abort signal.

## 8. Threat model

- **Assets.** Agent state consistency and the atomic handler commit (ADR 0109);
  the integrity of external side effects (payments, writes to other systems).
- **Hazards.**
  - Two effects issued together interleave in a way two sequential lines could
    not. Mitigation: the independence check (Q1) forbids one binding from
    depending on another, `store` writes are excluded from joins, and Q4's
    review confirms gates add no new interleaving.
  - Work keeps running after the handler replies, or is cut off half-done when
    the platform cancels it. Mitigation: settle-all (Q3) for joins and, after
    slice 0b, for the `parTraverse` family; an explicit `waitUntil` handover for
    anything a timeout abandons (Q6) or a race loses (Q7, amended), which is the
    only work allowed to outlive a join.
  - A handler reports a different error or fault depending on timing, which
    makes failures hard to reproduce and lets an attacker who can slow one
    dependency choose which error a caller sees. Mitigation: source-order
    reporting (Q2, Q3).
  - Retries under at-least-once delivery now replay several effects at once.
    This is no different from sequential code, which already re-runs every
    effect on retry, and the Idempotency capability applies unchanged.
- **Where verification happens.** The checker (block contents, independence,
  context), the emitted settle-all helper in the runtime library, and the
  behavioural fixtures for fault and error ordering.

## 9. Slice status

- [x] Slice 0 — concurrent binding block (#2640, shipped in #2646, ADR 0672)
- [x] Slice 0b — settle-all for the `parTraverse` family (#2662)
- [x] Slice 1 — `Result` and `?` in a join (dropped: the block already gives
  Q2's source-order rule; see Q2)
- [x] Slice 2 — agent handlers (folded into slice 0: the workerd gate test in
  #2646 found no new interleaving, so `par` stays legal in agent handlers)
- [x] Slice 3 — general timeout (#2683, shipped in #2693, ADR 0687)
- [x] Slice 4 — race (#2711, shipped in #2713, ADR 0691; adopts Q7 (b))

## 10. Non-goals

- **General cancellation** (Q7 (c)). It needs an abort signal through every
  capability, adapter and binding.
- **Background tasks that outlive the handler**, beyond `~>` and the timeout
  handover. `ctx.waitUntil` as a general surface is routed out of the standard
  library track as language/runtime work and is not taken on here.
- **Parallel CPU work.** A Worker isolate is single-threaded; this track is about
  overlapping *waits*, not computation.
- **Streams as a concurrency primitive.** `Stream[T]` stays pull-shaped and lazy.

## 11. Done when

- Every slice in §9 has shipped or been explicitly dropped with a recorded
  reason.
- The spec states the join surface, its independence rule, its fault and error
  semantics, the observation semantics and the timeout's guarantees normatively,
  in spec chapters 5 and 7.
- Sequential code emits byte-identically to before the track, proved by the
  existing goldens.
- Behavioural fixtures show that a join's result, its reported `Err` and its
  raised fault do not depend on completion order.
- The retirement PR removes this doc, appends its closing summary to
  `archive/retired-tracks.md`, and closes #2615.
