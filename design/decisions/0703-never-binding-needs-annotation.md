# 0703 — A binding that would infer the bottom type needs an annotation

- **Status:** Accepted (v0.481.6)

**Context.** `unreachable` and `todo` (#2620) have the internal bottom type `Never`, assignable to every type. `let x = todo("…")` with no annotation bound `x` to `Never`, so `x` silently stood in for any type, and an inlay hint would offer `never returns`, which is not source.

**Decision.** `Never` is inferred only where an expected type or a join gives the fault a type. An unannotated `let` (or `<-`) binding named other than `_` is refused with `bynk.types.never_inferred` when the value's type is or contains `Never`. A type built from `Never` (`List[Never]`, `Option[Never]`) is refused the same way. `let y = id(todo("…"))` is refused earlier, by generic inference (`bynk.generics.uninferable_type_arg`), so it never reaches this check. A faulting lambda is refused only when an unannotated `let` binds it. A faulting lambda passed to a combinator takes its type from the position it appears in for `map`, `mapErr` and similar; combinators that inspect the lambda's return shape (`flatMap`, `traverseTry`) still reject it with `bynk.types.combinator_return_mismatch` (pinned by fixture `2727_flatmap_faulting_lambda`; widening them touches a dozen shape checks and is left out). Faults in `if`/`match` branches (joined with the other branches), in tail position and against an annotation are unaffected. The failed binding takes the error type so its uses do not cascade. The inlay-hint sink drops any label that would render `never returns`.

**Consequences.** Authors annotate such a binding (`let x: Int = todo("…")`). A `let _ = …` stays allowed. Editors never offer the bottom type as source.
