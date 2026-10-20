---
title: The testing philosophy
---
Testing is built into Bynk rather than bolted on: `suite`/`case` blocks, `expect`,
`property`/`for all`, `Val[T]`, the `as <tier>` dial, and `stub` are language
constructs. This page explains why they exist in the form they do.

## One predicate surface

Bynk already has a way to state a checked claim: an **invariant** is a pure `Bool`
predicate — `is`, `implies`, the operators, pure methods — enforced at the commit
boundary. A test's `expect` is *that same predicate*, aimed at a value instead of a
committed state:

```bynk,ignore
expect    balance >= 0          // in a case
invariant nonneg: balance >= 0  // on an agent
ensures   nonneg: result >= 0   // on a function
```

There is no second assertion grammar and no matcher library to learn — one
predicate surface across production code and tests (ADR 0144). Moving from writing
code to verifying it introduces no new vocabulary; a failure reports the structure
of the predicate — `expected` versus `actual` — because there is one predicate
shape to render.

## Tests are part of the language, not a library

Because tests are a language construct, the compiler understands them. `expect` is
valid *only* inside a `case` — used anywhere else it is a compile error
(`bynk.expect.outside_case`), so test-only logic can never leak into production
code. The same is true of `Val[T]`. This is the type-system philosophy turned on
the test suite: the boundary between test and production code is enforced, not
merely conventional.

## Supplied vs generated subjects

A test needs subjects to check. There are two honest ways to get them, and Bynk
gives each its own construct.

A **`case` supplies** its subjects: you write the value the check is about, and
`expect` states the claim. When you need *a* value of some type without caring
about its exact contents, `Val[T]` fabricates one for you — for a refined type one
that satisfies the refinement; for a sum a variant; for a record every field. This
is deliberately *different* from real construction: it is an admission that "the
specific value is irrelevant here". When the value *is* relevant, you pin it —
`Val[T](50)` — and the pin is checked against the type's refinement just as a
literal would be.

A **`property` generates** its subjects: `for all x: T` draws inhabitants of `T`
from its refinement domain — boundary values included — and the body's `expect`s
must hold across all of them. Generation is *real*: the same refinement that a
`Val[T]` satisfies once, a `for all` samples across, so a property states a claim
about a *range* of inputs rather than one. This is where a `case` and a `property`
divide — one names a scenario, the other quantifies over a domain.

Some values cannot be generated or fabricated blindly — there is no sensible way to
invent a string matching an arbitrary regular expression — so a bare `Val` (or a
`for all`) over a `Matches`-refined type is rejected
([`bynk.val.needs_pin`](/book/troubleshooting/val-errors/)) and you must supply
one; an agent, which has no domain to draw from, cannot be generated at all. The
language would rather stop than guess.

## Contracts: the claim that is always on

Between a witnessing `case` and a quantifying `property` sits a third rung — the
**contract**. A claim about *one* result of a pure function does not belong in a
separate test at all; it belongs on the function, as an `ensures`. Bynk then
checks it everywhere for free: at every call in the dev/test build (a guard that is
stripped from production, so it costs nothing to ship), and by the runner, which
generates arguments — filtered by the function's `requires` — and attacks the
`ensures` exactly as a `property` attacks its body. A contract is a property that
is always on.

This sharpens the division of labour. A `case` witnesses a named scenario; a
contract states what one result always guarantees; a `property` earns its keep
only when the claim is *relational* or *spans calls* — a monotonicity, a
round-trip — which no per-call postcondition can express. A `case`/`property` that
merely restates a contract is redundant, and Bynk says so
([`bynk.contract.restated_by_test`](/book/troubleshooting/contract-errors/)). The
same one predicate surface runs at each rung: `case`, contract, `property`, and
`invariant` are the *same* predicate, checked over different subjects.

## Steps: the invariant that spans a commit

An `invariant` constrains a single committed *state*; the next rung constrains the
*move* between two — a **`transition`** over the `old`/`new` state pair, declared on
the agent beside its invariants (`old.status is Paid implies new.status is Paid`).
It is the same predicate surface again, now over a step, and it is checked at the
commit boundary from the second commit onward — so, like an invariant, it is carried
by the code and inherited by every `case` for free, at every tier, with no test
written for it. Unlike a contract's `ensures`, a transition is *not* attacked by the
runner: a fabricated agent state is valid but not necessarily reachable, so
attacking a step soundly means driving the real handlers (a later rung), not
fabricating states. `value → domain → call → snapshot → step` — one predicate, a
widening subject.

## Tier is a dial, not a kind

A unit under test can run with more or less of the real world behind it. The
testing pyramid names three amounts — `unit` (collaborators doubled),
`integration` (real collaborators within one context), `system` (contexts wired
across the real serialise → JSON → deserialise edge) — and the instinct everywhere
else in Bynk is to treat these as **one thing with a setting**, not three
different artefacts. So a tier is an `as <tier>` clause in the header, not a
separate construct: `unit` is the default and elided, and promoting to
`integration` is a **one-word header edit** with a byte-for-byte identical body.
(`system` drives deployed Workers by context path, so it is a suite of its own.) "Did I stub this
faithfully?" becomes a checkable question — promote the case and see whether the
real collaborator's invariants, which a stub was standing in for, still pass. The
participants of a higher tier are **inferred** from the `consumes` graph the
compiler already builds, so there is nothing to list and nothing to drift.

## Isolation: a stub is a test-scoped provider

A unit depends on collaborators — capabilities it asks for with `given`. Real
implementations may be slow, non-deterministic, or have side effects you do not
want in a test. A test double is simply an **alternative provider of a
capability** — it substitutes at the very seam the design already has, exactly
where production spells "supply an implementation" with `provides`. It uses the
same seam and the same wiring; the test does not reach around the design. What it
does *not* share is the keyword: a test double is written **`stub`**, its own word
since the keyword-hygiene batch (#548), so the reader never has to disambiguate a
three-way pun on `provides` (a provider declaration, an external provider, and —
formerly — a test double) by squinting at the shape that follows.

This completes a **seam triad**: `consumes` *declares* a seam, `given` *requires*
it, and `provides` *supplies* it in production — while `stub` supplies it, scoped
to a case or suite, in a test. A `stub` names the method with a call pattern (the
one predicate surface — `_`, literals, `is`) and returns a *value* or `fails`, never a
computed body: a double that wants logic is the signal to change tier, a deliberate
friction that stops a stub growing into a parallel program. (This is why the old
`mocks` re-implementation block — a whole alternative body — is gone; with it, the
word "mock" leaves the language.)

At `unit`, the tier's *intent* is that every collaborator is doubled; in v0.118 an
un-overridden seam still keeps its real provider, and a `stub` clause overrides
one seam. Full `unit` auto-stubbing — a synthesised return for every collaborator,
surfaced in the trace — is a **documented follow-on**, not yet enforced; the
distinction between `unit` and `integration` today is the *default provision
discipline* an author follows, not a compiler-enforced auto-stub.

## Observation: recorded, not spied

That same seam gives observation for free. To assert *that* a capability was called
— with what arguments, how often, in what order — you write nothing to arrange it:
because the capability is injected at a known seam, the test build records its calls
automatically. A pure-observation `case` supplies no `stub` at all; it just states
`expect Logger.log called once with msg == "…"` or `expect Store.put never called`.
This is the opposite of a spy library, where you install and configure the recorder;
here the recording is ambient, and the assertion is the *same* predicate surface —
`with <pred>` is the invariant predicate over the call's arguments. The sugar stays
small (presence, count, argument shape, order) because everything richer is
`trace(Cap.op)` plus the ordinary `List` surface — the vocabulary never has to grow
to stay expressive. `value → domain → call → snapshot → step → interaction` — one
predicate, a widening subject.

## Histories: sequence-generation, not state-fabrication

The top rung judges a whole *run*. A `for all run: History[Wallet]` generates a
bounded sequence of the agent's handler calls and binds `run` — an ordered
`List[Step]` — for the predicate to judge: "no accepted spend without a prior
accepted top-up" is `run.all(...)` over that list, with `run.upTo(s)` for "before".

The load-bearing idea is *how* that run is produced. A `property` over values could,
in principle, fabricate an agent state that satisfies every invariant — but a
valid state is not necessarily a **reachable** one. A balance of £50 with no
recorded top-up satisfies `balance >= 0` yet can never arise from the real handlers,
and a counterexample built from it would be a phantom. So a history is generated by
**driving**: the runner starts a fresh agent, invokes the *real* handlers with
generated arguments, and observes the states they reach. Validity is not
reachability; only driving gives reachability, and only reachable counterexamples are
worth reporting. This is why the history rung generates *sequences of calls*, never
*states* — the single most important idea in the slice.

Because it drives the real handlers, a history property needs no temporal logic: a
run is a finite `List[Step]`, and "always / eventually / before" are just
`.all` / `.any` / `run.upTo`. That finiteness is also the honest ceiling — a history
property is a runner *sample* over bounded runs, never a proof of unbounded liveness.
It is the runner-adversarial half of the duality whose commit-boundary half is
`invariant` / `transition`: the same reached states, judged from the other side.
`value → domain → call → snapshot → step → history` — one predicate, every rung
realised.

## The throughline

Test-only constructs are *checked* to be test-only; fabricated values are
*honestly distinct* from real ones; a tier is *a dial, not a kind*; a collaborator
is substituted *through the real seam* with `stub`; and behaviour is generated
by *driving*, not *fabricating*. Testing in Bynk follows the same instinct as the
rest of the language — make the safe thing structural — applied to how you verify
your code.

## See also

- Tutorial: [Test it](/book/tutorials/06-testing/).
- How-to: [Write tests and stub collaborators](/book/guides/testing/write-tests/).
- Guide: [Test tiers](/book/guides/testing/integration/).
- Reference: [testing](/book/reference/testing/).
