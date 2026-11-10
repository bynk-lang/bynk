---
level: minor
changelog: A direct self tail call runs in constant stack, emitted as a loop, for pure and `Effect` functions alike; `requires` is re-checked on each iteration, a function with `ensures` is not rewritten, and mutual and non-tail recursion keep a host-defined depth ([#3193](https://github.com/bynk-lang/bynk/issues/3193), [#3176](https://github.com/bynk-lang/bynk/issues/3176))
---

## ADR: self-tail-calls-run-in-constant-stack
title: A direct self tail call runs in constant stack
summary: The emitter lowers a direct self tail call to a loop and the spec guarantees it, reversing the tail-call half of #2612

**Context.** Bynk has no loop statement, so an accumulator-passing function is how
a program writes the loop the collection kernel does not fit (a parser, a state
machine, a cursor-paged fetch). #2612 (closed by #2655) specified recursion and
answered the tail-call question "no, deliberately": §7.3.2 said no tail-call
elimination was performed or guaranteed, since V8 does not implement proper tail
calls, so the depth of every recursion was host-defined. That left a trap:
`count(100000, 0)` type-checks, passes its small tests, and faults with a stack
overflow on the first large input (#3176). Proposed in #3193.

**Decision.** This reverses the tail-call half of #2612 and keeps the rest
(non-tail and mutual recursion are unchanged; their depth stays host-defined, as
`Float` arithmetic is under ADR 0042). #2612's reason concerned the host, but a
compiler can give by construction what the host never will, and "a direct self
tail call uses no additional stack" is a property of the emitted shape, not a
depth. The choices, from the proposal:

- **A. A guarantee, not an optimisation, for direct self tail calls only.** An
  optimisation that "may" apply moves the cliff without removing it. The spec says
  an emitter MUST NOT consume stack in proportion to the number of such calls.
- **B. What counts is syntactic and conservative.** The callee is the enclosing
  function itself (a free function by name, a method by receiver or qualified
  name) at its own type arguments, in tail position: the body's final expression,
  through `if` branches, `match` arms (guarded included) and a block's final
  expression. Not tail position: an argument or operand, under `?`, inside a
  lambda, `par`, `within` or `race`, bound by `let`, or wrapped. A call at a
  different instantiation of a generic function is not rewritten, because the loop
  variables would no longer have one TypeScript type. Agent and service handlers
  are untouched.
- **C. Contracts.** `requires` is re-checked on each iteration, as each recursive
  call checked it. A function with an `ensures` clause is not rewritten, in either
  profile: a loop has one frame, so the postcondition could only be checked for
  the outermost call, which is strictly weaker, and the two profiles would
  otherwise differ in stack behaviour. A lint nudging the author is out of scope.
- **D. Bindings are per iteration.** Closures capture variables, so reassigning
  parameters in place would let a lambda created in one iteration see the next
  one's values. The body sees fresh `const` bindings taken from loop variables at
  the top of each iteration; the call evaluates every new argument before
  assigning any loop variable (so a swap is correct) and `continue`s a labelled
  loop (so it passes through an emitted `switch`).
- **E. `Effect` functions are included.** The same rewrite applies to an `async`
  function; the `await`s ahead of the call are unchanged, so effect order is
  unchanged and the promise chain no longer grows by one per level.
- **F. Mutual recursion is out of scope.** A trampoline needs a runtime
  representation of a suspended call and changes every function in the cycle; it
  is a separate feature.
- **G. No lint** for a non-tail self call over a `List` (heuristic, advisory).
- **H. No `@tailrec` annotation** that errors when a function is not
  tail-recursive: it would add surface and a diagnostic, while the book now makes
  the rule knowable.

**Consequences.** Some stack-overflow faults become results; no program that
succeeded changes meaning. There is no grammar, checker or runtime change: the
tail-position analysis and the loop wrapper live in the emitter. The emitted
TypeScript for such a function no longer mirrors the source's call structure
(emitted TypeScript may still improve within 1.x; only the guarantee binds). The
guarantee binds every future emitter backend, which is the cost of choosing A.
Depth through mutual recursion, non-tail recursion, a call through a function
value, a shifting instantiation and an `ensures` function remains host-defined,
and the Book's "Recursion and iteration" guide says so. Hover, completion,
semantic tokens and signature help are unchanged, because no syntax, name or
signature is added.
