# 0171 — `List.parTraverse`, the concurrent sibling of `List.forEach`

- **Status:** Accepted (v0.147)
- **Provenance:** the deferred parallel form named in
  [ADR 0170](0170-do-statement-implicit-unit-and-list-foreach.md) Decision E
  ("The parallel form is deferred: `parTraverse` already covers the fan-out
  case"). Built directly on request; no separate proposal issue.
- **Realises:** a `List` may run an effect over every element **concurrently**
  (`parTraverse`), the fan-out counterpart to the sequential `forEach` that
  shipped in ADR 0170 — so a batch of independent effects (notify N subscribers,
  probe N targets) issues at once instead of awaiting each in turn.
- **Relates:** ADR 0170 (`List.forEach`, the sequential sibling this mirrors),
  ADR 0135 (`Query.parTraverse` — the concurrent broadcast terminal whose name,
  signature, and lowering this reuses), ADR 0116 (the eager `List` combinator
  vocabulary shares names with the lazy `Query`), ADR 0031 (effectful function
  values are confined to effectful contexts), ADR 0156 (the editor surface
  tracks the language).

## Context

ADR 0170 added `List.forEach(f: T -> Effect[()]) -> Effect[()]`, run **in
order**, and explicitly deferred the parallel form: sequential await is the
default a durable write path wants, and `Query`/storage already carried a
concurrent `parTraverse` (ADR 0135) for the WebSocket broadcast case. But an
eager `List` had no way to say "these N effects are independent — issue them at
once". A batch fan-out (notify every subscriber, fire N probes) either ran
serially through `forEach`, paying the sum of the latencies, or was hand-lowered
outside the language. `Query` had `forEach`/`parTraverse`; `List` had only half
the pair. This closes it.

The work is a one-arm extension, not new machinery: the checker already types
`Query.forEach`/`parTraverse` identically and the emitter already lowers the
`Promise.all` fan-out — `List.parTraverse` reuses both.

## The surface

`List.parTraverse(f: T -> Effect[()]) -> Effect[()]` — run `f` over every
element concurrently and await them all. The signature is identical to
`List.forEach`; the difference is purely execution order.

```bynk,ignore
// sequential: each awaited in turn
do subscribers.forEach((s: Sub) => notify(s))
// concurrent: all issued at once, awaited together
do subscribers.parTraverse((s: Sub) => notify(s))
```

## Decisions

**A — The name is `parTraverse`, mirroring `Query.parTraverse`; not
`parForEach`.** ADR 0116 established that the eager `List` and the lazy `Query`
carry the *same* combinator names, and `Query` already spells its concurrent
effect terminal `parTraverse` (ADR 0135). A developer who knows the `Query`
surface expects the identical pair on `List`; inventing `parForEach` for `List`
alone would split the vocabulary. The mild internal tension — `List.traverse`
(the stdlib helper) *collects* results into `Effect[List[B]]` while
`parTraverse` *discards* — is inherited verbatim from the `Query` naming
(ADR 0135) and is resolved the same way: the **collecting** parallel form is a
distinct future method, `parTraverseAll` (already on the roadmap alongside
`traverseAll`), not a rename of this one.

**B — Identical type to `forEach`; the difference is sequential-vs-parallel
lowering only.** `parTraverse` takes the same `f: T -> Effect[()]` and returns
the same `Effect[()]`. The checker arm is literally merged with `forEach`
(`FOR_EACH | PAR_TRAVERSE`), exactly as `Query`'s is — one signature, one
effectful-context gate (`bynk.effect.fn_value_in_pure_context`, ADR 0031), one
arity check. No new diagnostic: every misuse `forEach` already reports,
`parTraverse` reports identically.

**C — Lowers to `await Promise.all(xs.map(f))`, emitted inline.** The `List`
analogue of `Query.parTraverse`'s lowering: an `async` IIFE issuing every
element's effect eagerly and awaiting them together, so a slow element does not
head-of-line-block the rest. Inline, like `forEach`/`foldEff` — no runtime
import, and a file that never calls it emits byte-identically. The order in
which side effects *interleave* is unspecified (that is the point); the call
still completes only when all elements have.

**D — Sequential stays the default; the two are peers, not a replacement.** ADR
0170's reasoning holds: a durable write path that must apply effects in order
keeps `forEach`. `parTraverse` is the opt-in for genuinely independent effects.
Neither subsumes the other, so both ship.

## Consequences

- `List` and `Query` now carry the same effect-terminal pair
  (`forEach`/`parTraverse`) — the vocabularies line up.
- No new diagnostic, no grammar change, no runtime change; `parTraverse` is a
  new kernel-method name only.
- No emission churn for existing programs — net-new inline emission; `forEach`
  and `foldEff` lowering are untouched.
- The collecting parallel form (`parTraverseAll`) and its sequential peer
  (`traverseAll`) remain the named next slice.

## Tooling (ADR 0156)

- **Hover:** `List.parTraverse` hovers via the same kernel-method registry
  (`methods_for`) as `forEach`/`foldEff`.
- **Completion:** offered from `LIST_METHODS` alongside `forEach`.
- **Semantic tokens:** unchanged — `parTraverse` is an ordinary method name, no
  new token kind (no grammar/keyword change).
- **Signature help:** `parTraverse(f: T -> Effect[()]) -> Effect[()]` from its
  `LIST_METHODS` entry.
- **Formatter:** unchanged — a method call renders as any other.

## Alternatives considered

- **`parForEach` (A).** Reads unambiguously as "forEach, in parallel", but
  splits the `List`/`Query` vocabulary that ADR 0116 unified. Rejected in favour
  of cross-type consistency.
- **A separate checker arm from `forEach` (B).** Rejected — the types are
  identical, so merging the arm (as `Query` does) removes duplication and
  guarantees the two stay in lockstep.
- **Making `parTraverse` collect results (return `Effect[List[()]]` / a valued
  form).** Rejected for v1 — the discarding form matches `Query.parTraverse` and
  the common fan-out need; the collecting form is the separately-named
  `parTraverseAll` slice.
