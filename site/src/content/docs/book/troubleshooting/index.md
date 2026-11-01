---
title: Troubleshooting
---
One page per common diagnostic — the cause and the fix. Search for the error
code you saw.

- [`bynk.generics.missing_eq_bound`](/book/troubleshooting/generics-missing-eq-bound/) — a generic compares a
  type parameter that is not declared `Eq`.
- [`bynk.generics.missing_key_bound`](/book/troubleshooting/generics-missing-key-bound/) — a generic keys a `Map` or
  `Set` by a type parameter that is not declared `Key`.
- [`bynk.generics.missing_ord_bound`](/book/troubleshooting/generics-missing-ord-bound/) — a generic orders a
  type parameter that is not declared `Ord`.
- [`bynk.refine.literal_violates`](/book/troubleshooting/refine-literal-violates/) — a literal didn't
  satisfy a refined type's predicate.
- [`bynk.agents.non_zeroable_state_field`](/book/troubleshooting/agents-non-zeroable-state-field/) —
  an agent state field can't be zero-initialised.
- [`bynk.val.*` errors](/book/troubleshooting/val-errors/) — `outside_test`, `needs_pin`, and
  related `Val[T]` fabrication and `property` errors.
- [`bynk.contract.*` errors](/book/troubleshooting/contract-errors/) — `requires`/`ensures`
  contract clauses: `result_in_requires`, `not_bool`, `impure_predicate`,
  `duplicate_name`, `restated_by_test`.
- [`bynk.transition.*` errors](/book/troubleshooting/transition-errors/) — step
  invariants (`transition` over `old`/`new`): `not_bool`, `impure_predicate`,
  `no_step_reference`, `duplicate_name`, `cross_agent_reference`.
- [`bynk.observe.*` errors](/book/troubleshooting/observation-errors/) — observation
  (`expect Cap.op called …`, `trace`): `not_a_seam`, `unknown_op`, `with_not_bool`,
  `impure_with`, `trace_outside_test`.
- [`bynk.tier.*` and `bynk.stub.*` errors](/book/troubleshooting/integration-errors/) —
  the tier dial (`as <tier>`) and `stub` test doubles: `tier.system_needs_wire`, `stub.not_a_seam`, `stub.unknown_op`,
  `stub.rhs_type`, `stub.bad_sequence`.
- [`bynk.history.*` errors](/book/troubleshooting/history-errors/) — history properties
  (`for all run: History[Agent]`): `not_an_agent`, `not_generable`, `outside_property`,
  `restates_invariant`.

For the complete list of codes, see the
[diagnostic index](/book/reference/diagnostics/).
