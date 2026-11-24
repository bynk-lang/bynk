# 0936 — A selected consumed type wins over an unselected standard-library export of the same name

- **Status:** Accepted (v0.677.1)

**Context.** #4211 added `CallOutcome`, `CallFailure` and `Refusal` to `bynk`'s exports. A context that selects its own `Refusal` from another context while consuming `bynk { Secrets, Clock }` silently lost it: the composer skipped the consumed export because `Refusal` was an unselected `bynk` name, so the standard library's enum took its place and `bynk.consumes.type_not_selected` fired on the user's own type.

**Decision.** Precedence for a type name is: this unit's own declaration, then a type the file explicitly selects from a consumed user unit, then an unselected export of a first-party unit, which is only ever reached to report `type_not_selected`. An unselected export never displaces, shadows or blocks a name that is otherwise bound. Selecting the same name from `bynk` and from a consumed context stays `bynk.consumes.name_conflict`, and the message now names the other source.

**Consequences.** Adding an export to `bynk` or another std adapter cannot break a program that already uses the name for its own or a consumed type. No existing program changes meaning.
