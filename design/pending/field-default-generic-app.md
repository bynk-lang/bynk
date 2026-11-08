---
level: minor
changelog: A record field whose type applies a user generic record or sum may declare a default, `page: Paginated[String] = Paginated {}` or `outcome: Outcome[Int] = Pending("queued")`; inside a generic record it may write no value of the record's own type parameter
---

## ADR: field-default-generic-app
title: Field defaults on a field whose type applies a user generic
summary: The default renderers read a generic's fields at the instantiation, so a concrete instantiation, a pass-through parameter and a generic sum take a closed-form default

**Context.** ADR 0676 Decision I admitted defaults that hold for every
instantiation (`Option[T] = None`, `List[T] = []`) but left a field typed by a
user generic out: both default renderers (`lower_field_default_wire` and
`lower_field_default_value`) rejected every `TypeRef::App`. Proposed in #2690.

**Decision.**

- **A. Concrete instantiations.** For an `App` field the renderers read the
  declaration's fields (`wire::record_inst_fields`) or variants
  (`wire::sum_inst_variants`) with the arguments substituted, then render the
  default against those types. One renderer serves generic and non-generic
  records and sums: the helpers take `(name, type, declared default)` lists, not
  an AST body. The omitted-field rule inside a default literal applies to the
  generic's own declared defaults, rendered at the instantiated types.
- **B. A parameter passed through** (`Paginated[T]` inside `Cache[T]`) needed no
  separate rule. The substituted types still name `T`, and `T` stands for no
  value, so a default that must write a `T`-typed value is the existing
  "does not hold for every `T`" error, while `Paginated {}` (every `T` position
  empty or `None`) holds.
- **C. Generic sums.** A variant's payload is read at the instantiation, so
  `Pending("queued")` at `Outcome[Int]` renders; a payload that is the record's
  own parameter is refused as in B.
- **D. The enclosing record's parameters are passed to the renderer** and treated
  as unresolved before `types` is consulted. This is defence only: the language
  already refuses a parameter that shadows a declared type
  (`bynk.generics.type_arg_mismatch`), so the shadowing the proposal worried
  about is unreachable.
- **E. Decoding is unchanged.** The wire default is parameter-independent by
  construction (no `T`-typed value), so the per-instantiation codec splices it
  as for the admitted cases, and the generic decode path (`Json.decode[Cache[T]]`
  in a function over `T: Codable`) applies it; a behavioural fixture covers both.
- **F. Cycles.** A recursive generic record is already refused in a record field
  (`bynk.generics.recursive_generic_at_boundary`), so a self-referential default
  is unreachable through a generic; a depth limit in the renderer reports
  instead of recursing, should that change.
- **G. No new diagnostic code.** `bynk.types.bad_field_default` stays; its
  description no longer says a user generic has no default.

**Consequences.** An options record can default a field to a generic record or
sum. Defaults are not part of the contract hash (ADR 0676 E), so nothing about
hashes changes. Hover, completion, semantic tokens and signature help are
unchanged: no syntax or signature changes, and hover already renders a field's
default.
