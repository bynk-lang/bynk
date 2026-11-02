---
level: patch
changelog: An unannotated `let`, `<-` binding or lambda result whose value only faults (`unreachable`, `todo`) is refused with `bynk.types.never_inferred`, and no inlay hint offers `never returns`
probes: diagnostic_coverage, emit_diagnostics
---

## ADR: never-binding-needs-annotation
title: A binding that would infer the bottom type needs an annotation
summary: Where `Ty::Never` may be inferred, and the diagnostic that refuses it elsewhere

**Context.** `unreachable` and `todo` (#2620) have the internal bottom type `Never`, assignable to every type. `let x = todo("…")` with no annotation bound `x` to `Never`, so `x` silently stood in for any type, and an inlay hint would offer `never returns`, which is not source.

**Decision.** `Never` is inferred only where an expected type or a join gives the fault a type. An unannotated `let` (or `<-`) binding named other than `_`, and a lambda whose result is not fixed by an expected function type, are refused with `bynk.types.never_inferred` when the value's type is `Never`. This covers `let y = id(todo("…"))`, whose generic result has no expected type. Faults in `if`/`match` branches (joined with the other branches), in tail position and against an annotation are unaffected. The failed binding takes the error type so its uses do not cascade. The inlay-hint sink drops any label that would render `never returns`.

**Consequences.** Authors annotate such a binding (`let x: Int = todo("…")`). A `let _ = …` stays allowed. Editors never offer the bottom type as source.
