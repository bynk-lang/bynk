---
level: patch
probes: emit_diagnostics, diagnostic_coverage, explained_codes
changelog: An `expect` predicate and an observation's `with` predicate may not hold an effectful call (`bynk.expect.impure_predicate`, `bynk.observe.impure_with`), and an `expect` comparison evaluates each operand once, so a capability call can no longer be recorded twice by a claim about it
breaking: An `expect` predicate that holds an effectful call, such as `expect [Logger.log("x")].length() == 1`, is now `bynk.expect.impure_predicate`, and an effectful call in a `with` predicate is now `bynk.observe.impure_with`. Run the call first with `let x <- …` and state the claim about `x`.
---

## ADR: expect-predicates-hold-no-effect
title: An `expect` predicate and an observation's `with` predicate hold no `Effect`
summary: The test predicate positions enforce ADR 0144 D3 by type, not by syntax

**Context.** ADR 0144 D3 bars effects and capabilities from every predicate position, and ADR 0145 D2 makes `expect` "the invariant predicate, checked identically"; ADR 0145 D6 rests the expected-vs-actual display, which evaluates a comparison's operands for the failure message, on that purity. But only `bynk.expect.not_bool` guarded `expect`. The purity gate the other positions share, `predicate_impure_construct`, is syntactic: it refuses `Effect.pure`, `?`, `expect`, `Val`, observations, `faults` and `trace`, and relies on those positions having no capability in scope. A `case` body has its capabilities in scope, so an `Effect` held inside a pure expression type-checked: `expect [Logger.log("x")].length() == 1`, or one returned by a lambda in a `map`. A capability call lowers to an eager call on the case's recording `deps`, so the claim ran the call and the recorder (ADR 0152) logged it. Combined with the comparison's double lowering, a `?` operand that held one was recorded twice on a passing claim, and `expect Logger.log called once` failed on a correct program (#3768). An observation's `with` predicate, checked by the same syntactic gate and evaluated once per recorded call, had the same gap.

**Decision.**

- **The rule is by type.** After an `expect` predicate is typed, no sub-expression of it may have type `Effect[_]`; the first one is `bynk.expect.impure_predicate`, labelled at that sub-expression. This covers capability, service and agent calls and effectful functions alike, with no list of call shapes to keep in step with the language. Typing already refuses an `Effect` that is compared, matched or unwrapped; this closes the remaining case, an `Effect` held as a value.
- **`expect`'s own constructs are not walked.** A fault claim (`expect <call> faults`) awaits its call by design (#1706); an observation's `Cap.op` subject is named, not called; `trace(Cap.op)` reads the recording. `?`, `is`, `Val` and pure lambdas have no `Effect` type and are unaffected.
- **"No test-only constructs" governs the other positions.** `expect` is where the test-only constructs live, so for `expect` ADR 0144 D3 means no effects and no capabilities. The syntactic gate is not applied to it.
- **The `with` predicate gets the same rule under its own code.** An `Effect`-typed sub-expression of a `with` predicate is `bynk.observe.impure_with` (ADR 0144 D4: one code per position), reported once: the syntactic gate, run first, wins.
- **The comparison evaluates each operand once.** Independently of the rule, an `expect` comparison lowers each operand once; when either hoists statements, both are bound to temporaries after the hoists and the condition and the failure detail read them (#3769), so the emitted test does not depend on purity for evaluating an operand once.
- **Under the feature freeze (ADR 0876) this is a defect fix.** The purity was already required by ADR 0144, ADR 0145 and the specification (§5.9); the change adds a diagnostic, not syntax or API. No fixture or example in the repository was refused by it.

**Consequences.** A test states its claims about values that are already computed: an effectful call runs in a `let x <- …` before the `expect`, which is also where the recorder expects to see it. The emitter's double evaluation of a comparison's unhoisted operands on the failure path remains, and is sound because the predicate is now pure. The other predicate positions keep the syntactic gate, which is enough where no capability is in scope; a future position that brings capabilities into scope should adopt the typed rule.
