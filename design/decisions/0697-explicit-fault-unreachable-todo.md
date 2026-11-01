# 0697 — A program can fault deliberately — `unreachable` and `todo`, with a bottom type

- **Status:** Accepted (v0.479)

**Context.** Faults were first-class (ADR 0444 — a fault is claimed, not caught; the
`expect … faults` claim) but only the runtime could raise one: division by zero, an
invariant violation, a rehydration violation. A program could not say "this arm is
impossible", so the author wrote a made-up default that silently masked a broken
invariant, and had no placeholder for unfinished code that still type-checks. Bynk had
no bottom type; `if`/`match` branches join by least upper bound (§5.2).

**Decision.** (Issue #2620, all six decisions at the proposal's Recommended option.)

- **A.** An internal `Never` type, joined with every type (`join(Never, T) = T`) and
  assignable to every type, never nameable in source. It displays as "never returns".
- **B.** Spelled as ambient built-in functions `unreachable(message: String)` and
  `todo(message: String)`, not keywords: no grammar change, and a declaration of the same
  name (a `fn`, parameter or local) shadows them, as `Json` does. Recorded as
  `Callee::Fault`.
- **C.** `todo` compiles, with the non-failing warning `bynk.fault.todo` (ADR 0117), so
  `check --deny-warnings` can refuse it in CI.
- **D.** Both lower to the runtime's `never`-returning `__fault(kind, message, location)`,
  with kinds `Unreachable` / `Todo`, and the call-site `path:line:col`. It throws, so it
  rides the existing uncaught-fault channel: the handler faults, an agent commit does not
  happen (ADR 0109), and the response is the existing fault response.
- **E.** No general `panic`: two named, intent-revealing forms keep errors-as-values honest.
- **F.** Valid in pure and effectful positions; in an `Effect[T]` position the fault
  happens when the effect runs (a throw inside the async body), and `x <- todo(…)` is
  accepted, binding `x` at `Never`.

**Consequences.** An `unreachable` arm type-checks in a `String`-valued `match` without a
placeholder, and `expect … faults` observes it. Exhaustiveness is unchanged (the arm still
counts). Editor tooling shows the built-ins' signature and doc (hover, completion,
signature help); semantic tokens are unchanged (ordinary calls). `Never` is expression-only:
a lambda whose body is only a fault call takes its type from the expected function type.
The proposal's risk that `Never` leaks into inferred types is not yet closed for an
unannotated `let x = todo("…")`, which infers `Never`; tightening it (#2727), and folding the
fault's kind and location into the typed agent-fault channel when that lands, are
follow-ups.
