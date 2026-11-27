---
level: minor
probes: diagnostic_coverage, emit_diagnostics, explained_codes
changelog: A commons function forwards a capability parameter to another by binding it to its own, `bottom(key, given Inner = Store)`, through chains, generic and `Ord`-bounded functions and recursion; a function with capability parameters named as a value is `bynk.given.parameterised_fn_value`, and a lambda that binds takes its place (experimental, behind `bynk.given.parameters`)
---

## ADR: capability-parameters-forwarding
title: A commons function forwards a capability parameter by binding it to its own, and a parameterised function is not a value
summary: Forwarding is the ordinary binding rule applied in a commons body; a value of a parameterised function is refused in favour of a lambda that binds

**Context.** #3959, slice 2 of the capability-parameters track (`design/tracks/capability-parameters.md`, §7 and Q5, Q8). Slice 1 checked and emitted a binding from a context or a provider body. A library is usually several commons functions, so a parameter has to pass from one to the next, and the track records forwarding in this slice.

**Decision.** (A) Forwarding is not a separate rule. A commons function binds its callee's parameters with the same in-argument binding (`given Inner = Store`). Its own parameters are capabilities in its scope, of their bound kind, so the slice-1 checks apply unchanged: the right-hand side must be in the caller's `given`, of exactly the bound's kind, and counts as used. A commons body sees no instance, so it can bind only to its own parameters. Names never forward implicitly (Q5), and that would be inference by another name. (B) The check runs per call, so it holds along a chain with no graph walk, beside type parameters and `Ord` bounds, whose comparator witnesses still precede the hidden `deps`. A binding missing three deep is reported at the call in the function that leaves it out. (C) Recursion needs no special case in the checker. A self tail call is emitted as a loop only when it binds every parameter to itself, because the loop keeps the function's own `deps`; a call that rebinds (`swap(n - 1, given A = B, B = A)`) stays a real call, which builds the `deps` it names (the guard landed with slice 1, which could already reach such a call). (D) A function with capability parameters named as a value is `bynk.given.parameterised_fn_value` (Q8). ADR 0726 lets a `given` function close over its caller's `deps` because nothing needs choosing; here a binding must be chosen, and a lambda that binds already says which. A function with only shared capabilities is still a value, as a context's is.

**Consequences.** Libraries can be written as several small functions over one instance. A lambda's binding is explicit at the place the value is made. Partial application of bindings stays out of the language. Slice 2 changes no emission.
