---
level: minor
changelog: `.of` on a named type with no predicate is total and returns the type; "alias" is renamed "named type"
breaking: `T.of(x)` on a named type with no predicate (`type Id = Int`) now returns `T`, not `Result[T, ValidationError]`, so `Id.of(n)?` or a `match` on it stops compiling (`bynk.types.question_on_non_result`, with a machine-applicable "drop the `?`" fix-it); the emitted TypeScript `Id.of(n)` returns the branded value, not a result object. Adding a predicate later turns `.of` back into a `Result`. A `Float` named type stays fallible.
---

## ADR: named-type-total-of
title: Named types: `.of` is total when the type has no predicate, and "alias" is renamed "named type"
summary: A predicate-free named type's `.of` returns the type; the form is called a named type, not an alias

**Context.** `type Id = Int` was documented as an "alias", but it is a distinct, branded nominal type whose only run-time constructor, `Id.of`, returned `Result[Id, ValidationError]` although it could never be an `Err`. Call sites handled an impossible failure, and adding a predicate later changed nothing at them.

**Decision.** `.of` on a named type with no predicates (annotations alone are not predicates; opaque types are unchanged) is total: it types as `Id` and emits a brand cast with no checks. A type with predicates keeps `Result[T, ValidationError]`, so adding a `where` clause makes every unhandled call site a type error. The rule is "no predicate and no implicit base check": `Float` has one, its finite-number check (ADR 0040), which is part of the type, so a `Float` named type stays fallible. The form is called a *named type* in the book, spec, glossary and tooling; there is no transparent alias. `?` on a total `.of` carries a note and a fix-it dropping the `?`.

**Consequences.** Existing `Id.of(n)?` stops compiling with a one-step fix; host TypeScript that unwrapped the result object must change. Signature help and the `.of` completion detail show the new return type.
