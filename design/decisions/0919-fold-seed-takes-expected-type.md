# 0919 — An empty fold seed takes its type from the fold's expected result

- **Status:** Accepted (v0.663)

**Context.** `fold` typed its accumulator from `init` alone, with no expected type, so `posts.fold(Set.empty(), (acc, p) => acc.union(p.tags))` was `bynk.types.uninferable_element_type` even where the fold's result type was known (a typed `let`, a return type). The accepted decision on #4193 was to infer the accumulator from `init` and the step function together.

**Decision.** The checker has no inference variable: `Ty::Var` is a named generic parameter, not a unification variable, and `X.empty()` needs an expected type at the moment it is typed. Settling the element type from the step body would need that machinery. So this increment takes the documented fallback in part: the expected type of the `fold` call is passed to the seed (`Ctx::fold_init_expected`, set at the call site and taken by the `List`, `Set` and `Map` fold arms), so the typed `let`, return-type and argument positions work for all three; a fold with an empty seed and no such context still reports `bynk.types.uninferable_element_type`, and its note now names the fix (annotate the result, or pass a typed seed). The step function does not settle the element type. `foldEff` and the `Query` fold are not changed.

**Consequences.** Programs that were rejected for want of a type now compile where a type is expected; none that compiled change. The fully unannotated `let tags = xs.fold(Set.empty(), …)` is still an error, with a clearer note. Settling it from the step function is a larger checker change (unification variables) and is left for a later increment if wanted.
