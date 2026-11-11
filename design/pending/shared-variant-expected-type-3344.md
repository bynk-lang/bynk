---
level: minor
probes: explained_codes
changelog: A bare variant name that two sums in scope declare (two enums a consumed unit exports, say) builds the sum the expected type names, from an annotated `let`, a parameter, a return type or a `stub`'s operation, and is `bynk.resolve.ambiguous_variant` only when nothing picks one; the message names each qualified form, and a `stub` value no longer reports it as `bynk.stub.rhs_type` (#3344)
---

## ADR: shared-variant-by-expected-type
title: A variant name several sums share resolves by the expected type, never by picking one
summary: Expected type picks the owner; otherwise `bynk.resolve.ambiguous_variant`

**Context.** A bare variant name builds a value of the sum that declares it.
When two sums in scope declare it, the resolver reported
`bynk.resolve.ambiguous_variant` without reading any type, because it has
none. That refused `Err(Failed("x"))` in a body returning
`Result[_, MailError]`, where the type leaves no doubt. A `stub` value folded
the same error into `bynk.stub.rhs_type`, which names the wrong problem. This
surfaced in #3302: `bynk.cloudflare` exports `SqlError` with a `Failed` variant,
and an email error enum with its own `Failed` made every bare `Failed` an
error. ADR 0078 says built-in verdict variants inherit
`bynk.types.ambiguous_constructor`, but that code was withdrawn in v0.298.0
(#1659): one type always decides what `Ok` builds.

**Decision.** The checker, not the resolver, decides a bare variant's owner.
One sum in scope declaring the name is the owner. When several do, the expected
type picks the one it names (`Ty::Named`), whether it comes from an annotated
`let`, a parameter, a return type, a nested position such as `Err(…)`'s payload,
or the operation a `stub` overrides. When nothing picks one, it is
`bynk.resolve.ambiguous_variant`, which names every owner and its qualified
form. The compiler never picks one of several owners on its own. Patterns were
already scrutinee-directed and are unchanged. A `stub` value's ambiguity
surfaces under its own code, as an unresolved name has since #2907. The rule
covers user sums, a `uses`d commons's sums and a consumed unit's exported sums
alike, because all of them reach the checker's type table the same way.

**Consequences.** A first-party unit can add a sum whose variant name another
of its sums already uses without breaking consumers whose uses are typed by
context. Only an untyped use needs qualifying, and it says how. An expected
type that is an alias or a refinement of the sum does not pick an owner: the
compiler stays conservative and reports the ambiguity. ADR 0078's reference to
`bynk.types.ambiguous_constructor` is superseded by this record.
