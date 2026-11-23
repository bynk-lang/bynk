---
level: minor
probes: emit_diagnostics, diagnostic_coverage, explained_codes
changelog: A refinement over `Caller` (`actor Admin = Caller where IsContext(shop.admin)`) admits named calling contexts, checked at compile time (`bynk.actor.caller_not_admitted`, `bynk.actor.unknown_context`) and again at the callee (`403`)
---

## ADR: caller-refinement-is-context
title: Refinement actors over `Caller` admit named calling contexts
summary: `Caller` becomes a refinement base, `IsContext(…)` its predicate, decided against the project's call graph

**Context.** `by c: Caller` admits every context that consumes the callee. A
service meant for one or two contexts had to compare `c.identity` with a string
by hand: a typo compiled, and the restriction was invisible in the signature.
Refinement actors already narrow a base actor, but only a `Bearer` one. Who may
call is an access decision, which Bynk puts in the `by` clause (ADR 0082), and
the compiler sees every call edge in the project, so it can refuse a disallowed
call before deploy, as no check in the body can.

**Decision.**

- **A.** `Caller` becomes a refinement base, alongside `Bearer`. A refinement
  over `Caller` inherits its scheme (`Internal`), its identity (`CallerId`) and
  its admissibility (`on call` only).
- **B.** The predicate is `IsContext(<qualified name>, …)`: one or more context
  names written as names, resolved against the project's contexts
  (`bynk.actor.unknown_context` otherwise). It is capitalised, as a fixed,
  compiler-known check decided at compile time (the case rule proposed in #918),
  and admissible only over `Caller`; `hasClaim` and `claimEquals` stay `Bearer`'s.
  The existing combinators apply, so `!IsContext(shop.legacy)` is a deny-list.
- **C.** A project phase visits every cross-context service call. When the
  callee's handler is guarded by a `Caller` refinement whose predicate does not
  admit the calling context, it reports `bynk.actor.caller_not_admitted` at the
  call, naming the actor and the contexts it admits. The predicate is over
  constants, so it is decided exactly.
- **D.** The run-time `403` stays. Wire format check 6 evaluates the predicate on
  the proven caller, because signing keys are per pair of contexts and a caller
  from before the restriction may still be deployed.
- **E.** A `Caller` refinement may be a sum's one `Internal` member, so
  `by who: Fulfilment | Ledger` combines it with `Foreign` actors. A sum already
  has at most one `Internal` member (`bynk.actor.duplicate_sum_scheme`), so
  nothing needs ordering. Refinements over `Bearer` stay out of sums.
- **F.** In a test, a call-site `by Admin("shop.admin")` is typed against the
  refinement, and a literal context the predicate refuses is
  `bynk.actor.caller_not_admitted` in the case too. A `system` case still drives
  the `403`.

**Consequences.** No wire-format change: the `403` at check 6 is already in v1.
The deploy contract hash is unaffected, because admission is not part of the
contract. Contexts outside the project (reached through the registry) cannot be
named; foreign callers have their own actors. A restriction added later breaks an
existing caller at compile time, not in production. The call-edge check reads the
`Callee::Cross` sites the analysis keeps, so `bynk check` now retains them.
