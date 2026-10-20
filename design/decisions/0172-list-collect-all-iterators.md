# 0172 — `List.traverseAll` / `List.parTraverseAll`, the collect-all iterators

- **Status:** Accepted (v0.148)
- **Provenance:** the collect-all variants named in the design notes (§11, "In-memory
  collection iteration with effects") and reserved as the next slice by
  [ADR 0171](0171-list-partraverse.md) ("the collecting parallel form
  `parTraverseAll` and its sequential peer `traverseAll` remain the named next
  slice"). Built directly on request.
- **Realises:** running an effectful, *fallible* function over a `List` and
  gathering **every** outcome — `Ok` and `Err` alike — instead of stopping at
  the first failure. `xs.traverseAll(f)` (sequential) and `xs.parTraverseAll(f)`
  (concurrent) both take `f: T -> Effect[Result[U, E]]` and return
  `Effect[List[Result[U, E]]]`.
- **Relates:** ADR 0170 (`forEach`, the discarding sequential iterator), ADR 0171
  (`parTraverse`, the discarding concurrent iterator — the emission and
  effectful-context discipline this reuses), ADR 0116 (the eager `List`
  combinator vocabulary shared with `Query`, and the migration to kernel
  methods), ADR 0031 (effectful function values are confined to effectful
  contexts), ADR 0156 (the editor surface tracks the language). The stdlib
  `traverse` (sequential collect) is the short-circuiting sibling these
  complement.

## Context

Bynk's effectful `List` iterators covered "run and discard" (`forEach`,
`parTraverse`) and "run and collect, sequential" (the stdlib `traverse`). What
was missing is the **fault-gathering** case: a bulk operation where some
elements fail and the caller wants *all* the outcomes, not just the first error.
Form validation that reports every invalid field, bulk processing where partial
success matters, compensation tracking — all need "apply `f` to each, keep every
`Result`". The short-circuiting forms throw that information away at the first
`Err`.

The design notes (§11) named the pair — `traverseAll` / `parTraverseAll` — from
the outset, with the signature `f: A -> Effect[Result[B, E]] -> Effect[List[Result[B, E]]]`.
This increment builds exactly that, reusing machinery that already exists: the
checker's `check_kernel_fn_arg` infers a function argument's return type by
unification (as `map`/`flatMap` do), and the emitter already lowers the
sequential-collect (`foldEff`) and concurrent (`parTraverse`) shapes.

## The surface

```bynk,ignore
// gather every field's validation outcome, sequentially
let outcomes <- fields.traverseAll((f: Field) => validate(f))
// gather every probe's outcome, concurrently
let outcomes <- targets.parTraverseAll((t: Target) => probe(t))
// outcomes : List[Result[…, …]] — one entry per element, Ok or Err
```

Both are `Effect[List[Result[U, E]]]`; `traverseAll` awaits each element in
order, `parTraverseAll` issues them all at once.

## Decisions

**A — The collect-all forms take a `Result`-returning function and never
short-circuit; that is the whole point.** `f: T -> Effect[Result[U, E]]` →
`Effect[List[Result[U, E]]]`. Every element's outcome is appended to the result
list — in input order — whether it is `Ok` or `Err`. This is sound *and cheap* precisely because a
Bynk `Result` `Err` is a **value**, not a fault (errors-as-values): `f` resolves
to a tagged `Ok`/`Err` object, never a rejection, so a sequential loop simply
collects each and a `Promise.all` gathers them all without any element rejecting.
No `allSettled`, no fault interception (`attempt`/`recover` stays deferred) — the
language's error model already makes collect-all fall out for free.

**B — Kernel methods, not stdlib functions.** ADR 0116 set the direction: the
effectful iteration vocabulary lives as **kernel methods** on `List`, and the
recent `forEach` (ADR 0170) / `parTraverse` (ADR 0171) followed it. Making
`traverseAll`/`parTraverseAll` kernel methods keeps the pair symmetric
(`xs.traverseAll(f)` / `xs.parTraverseAll(f)`, the `parTraverse` call shape) and
lets them emit **inline** — no runtime import, no new stdlib unit. The legacy
stdlib `traverse` (a free function) is left as-is; migrating it to a kernel
method is a separate, out-of-scope cleanup.

**C — The function's return type is inferred, then required to be
`Effect[Result[U, E]]`.** The checker uses the existing `check_kernel_fn_arg`
(unification over a `__kernel_ret` type variable, as `map` uses for its `U`),
then peels `Effect[Result[U, E]]` to the `Result[U, E]` the result list carries.
A function returning a non-`Result` effect (`Effect[U]`) is
`bynk.types.argument_mismatch` with a message naming the required shape — **no
new diagnostic code**, reusing the argument-mismatch the other function-arg
kernels raise. Like `forEach`/`foldEff`, each runs an effectful function value
and is confined to effectful contexts (`bynk.effect.fn_value_in_pure_context`,
ADR 0031).

**D — Emitted inline, mirroring the existing shapes.** `traverseAll` is the
`foldEff` sequential-loop shape specialised to a push-collect into a typed
`Result<U, E>[]`; `parTraverseAll` is the `parTraverse` `Promise.all` shape that
*keeps* the resolved array instead of discarding it. Both yield
`Promise<Result<…>[]>`, `tsc --strict`-clean (the output array is annotated from
the checked type, as `foldEff`'s accumulator is). No runtime helper, no churn to
`forEach`/`parTraverse`/`foldEff`.

## Consequences

- The four-cell effectful-iteration matrix is now: discard×{seq,par} =
  `forEach`/`parTraverse`; collect-all×{seq,par} = `traverseAll`/`parTraverseAll`.
  The remaining gap — the *short-circuiting* collect (`traverse`'s `Result`
  overload and a `parTraverse` collecting overload) — is a named later slice; it
  needs return-type-dependent **overload dispatch** on the same method name,
  which these `All` methods (single signature) deliberately avoid.
- No new diagnostic, no grammar change, no runtime change; two new kernel-method
  names only.
- No emission churn for existing programs — net-new inline emission; every other
  kernel's lowering is untouched.

## Tooling (ADR 0156)

- **Hover / Completion / Signature help:** both methods are offered and
  signature-helped from the `LIST_METHODS` registry (`methods_for`), like every
  other list kernel.
- **Semantic tokens / Formatter:** unchanged — ordinary method names, no grammar
  or keyword change.

## Alternatives considered

- **`traverseAll` as a stdlib function (built on `foldEff`).** It *is* expressible
  in surface Bynk (sequential), but `parTraverseAll` needs `Promise.all` and must
  be a kernel — splitting the pair across a free function (`traverseAll(xs, f)`)
  and a method (`xs.parTraverseAll(f)`). Rejected for the asymmetry; both are
  kernel methods (B).
- **`Promise.allSettled` semantics (gather runtime faults, not `Result`s).**
  Rejected — Bynk models recoverable failure as `Result` values, not exceptions;
  gathering *faults* is the deferred `attempt`/`recover` surface, a different
  feature. Collect-all here is over `Result`, matching the design notes.
- **A single overloaded `traverse` that switches collect vs collect-all on a
  flag or return shape.** Rejected — the design notes give the collect-all forms
  their own names ("no opaque suffix; each operation's role is in its name"), and
  distinct names keep the short-circuit-vs-gather choice explicit at the call
  site.
