# 0894 — Named types: `.of` is total when the type has no predicate, and "alias" is renamed "named type"

- **Status:** Accepted (v0.645)

**Context.** `type Id = Int` was documented as an "alias", but it is a distinct, branded nominal type whose only run-time constructor, `Id.of`, returned `Result[Id, ValidationError]` although it could never be an `Err`. Call sites handled an impossible failure, and adding a predicate later changed nothing at them.

**Decision.** `.of` on a named type with no predicates (annotations alone are not predicates; opaque types are unchanged) is total: it types as `Id` and emits a brand cast with no checks. A type with predicates keeps `Result[T, ValidationError]`, so adding a `where` clause makes every unhandled call site a type error. The rule is "no predicate and no implicit base check": `Float` has one, its finite-number check (ADR 0040), which is part of the type, so a `Float` named type stays fallible. The form is called a *named type* in the book, spec, glossary and tooling; there is no transparent alias. `?` on a total `.of` carries a note and a fix-it dropping the `?`.

**Consequences.** Existing `Id.of(n)?` stops compiling with a one-step fix; host TypeScript that unwrapped the result object must change. Signature help and the `.of` completion detail show the new return type.
