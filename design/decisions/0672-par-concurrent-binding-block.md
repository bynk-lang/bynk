# 0672 — A `par { … }` block joins independent effects, settle-all, with faults raised in source order

- **Status:** Accepted (v0.457)

**Context.** Every `<-` awaits, so two independent effects cost the sum of their
latencies, and an unawaited `Effect` is an error (`bynk.effect.unbound_effect`,
ADR 0426), so the JavaScript idiom of starting two promises and awaiting both
cannot be written. The concurrent-effects track (spine #2615) settled Q1 (c), a
statement-form binding block rather than a tuple type or a record-shaped join,
Q3's settle-all fault semantics and Q5's issue-order observation. This record
carries the decisions slice 0 (#2640) fixed.

**Decision.**

- **A. Keyword.** `par`, contextual like `advance`: it opens a statement only
  before `{` at statement position, and is an identifier everywhere else, so no
  existing program changes. **Rejected:** `concurrently`, `together` (longer; `par`
  matches `parTraverse`).
- **B. What the block holds.** One or more of `let x <- e` (optionally typed) and
  `do e`. Refused, each as `bynk.par.invalid_statement` naming the statement: a
  `let pattern <- e else …` (refutable, it would need a branch per binding), a
  plain `let` (no effect to overlap), `~>` (fire-and-forget has nothing to
  join), `:=` and a mutating store operation (`store` writes stay sequential,
  track Q4), `expect`, `advance`, a nested `par`, and an empty block. A `?`
  inside a binding is `bynk.par.question_in_join`: Q2's source-order rule is
  slice 1. The parser admits any statement but a `let … else` and the checker
  names the rest, so the editor still sees the rejected statement in place. A
  `?` inside a lambda belongs to the lambda and is not refused.
- **C. Sibling names.** No binding may mention a name another binding of the
  same block binds, and no two bindings bind one name (`bynk.par.sibling_name`).
  The rule holds even where the name would resolve to an outer binding, so
  `let user <- …; par { let user <- A(); let o <- B(user) }` is refused rather
  than silently passing the stale `user`. A binding's own name is not a
  sibling: `let orders <- F(orders + 1)` reads the earlier `orders`.
- **D. Scope.** The names enter the enclosing scope after the closing brace; each
  binding is checked against the scope outside the block. Re-binding an earlier
  `let` name follows the existing ML-style rule. The block has no value; it is a
  statement, legal only in an effectful body, like any `<-`.
- **E. A single binding** is `bynk.par.single_binding`, a non-failing warning
  (ADR 0117): it is a plain `let x <- e` with extra syntax, but can be a harmless
  intermediate state while editing.
- **F. Fault and value semantics: settle-all, source order.** Each binding's
  expression is evaluated in source order, producing promises. The block awaits
  all of them. On success every value is bound. If any rejected, the **first
  rejection in source order** is raised, after every binding has settled; later
  rejections are attached to it as `suppressed` for logging and are not
  catchable separately. A fast failure therefore waits for a slow sibling, the
  price of determinism and of keeping every awaited effect inside the handler's
  lifetime; it is the trade `parTraverseAll` makes. Lowered to
  `const [a, b] = await __bynkSettleAll([() => e1, () => e2]);`, with fresh
  identifiers through the existing re-binding mechanism; a `do` takes a hole in
  the pattern. Each binding is a thunk, called in source order by the join, with
  its hoisted statements inside it; a thunk that throws synchronously becomes a
  rejected entry, so an earlier effect is never left unawaited (the join still
  calls every later thunk). A typed binding types its thunk
  (`(): Promise<T> => …`) so the name has the type `const x: T = await e` gives
  it. Every effect is still awaited, so ADR 0426's
  guarantee holds. `__bynkSettleAll` lives in a new `settle` runtime module,
  imported only by a module with a `par` block, so sequential code emits
  byte-identically. It is codegen, **not** part of ADR 0310's published ABI.
- **G. Observation.** The recorder is unchanged: it logs each capability call
  when made, so the first capability call of each binding is recorded in source
  order, and `expect A.op before B.op` compares **issue** order. Later calls
  inside a composite binding follow completion order (track Q5); neither is
  evidence that one finished first. The specification and the testing reference
  say so.
- **H. Durable Object gates.** The track's Q4 assumed a join adds no
  interleaving that sequential awaits do not already allow. Checked on a real
  `workerd` (`bynkc/tests/par_gate_workerd.rs`): two requests to one agent, each
  reading a `store` field, making two subrequests and writing the field back,
  were driven through a sequential and a `par` handler, once overlapped and once
  serially. The `par` outcome (what each request read, and the final value) was
  in every schedule one the sequential form produces: overlapped, both read `0`
  and the field ends `1`, which the sequential form already does (an outbound
  `fetch` does not hold the input gate); serially, `0`, `1`, `2`. The order the
  subrequests reach the server differs by construction and is not part of the
  claim. So joins are legal in agent handlers, with the rules above, and the
  fallback (refusing `par` there) was not needed. Calling the agent's own
  handlers from inside a join stays exactly as legal or illegal as it is
  sequentially.

**Consequences.** Independent effects can overlap, and the wait sits at a visible
closing brace. The language gains no tuple type. The `parTraverse` family still
raises at the first rejection (`Promise.all`); aligning it to settle-all is
slice 0b. `?` in a join (slice 1) and a general `within` timeout (slice 3)
build on this block. The grammar's `par_stmt` holds every statement but a
`let … else`, a deliberately permissive subset the compiler narrows.
