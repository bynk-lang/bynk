# 0556 — `[dev-dependencies]` resolve with the graph, reach only suites, and never a deployable build

- **Status:** Accepted (v0.379)

**Context.** Slices 1 and 2a refused a non-empty `[dev-dependencies]` until this slice.

**Decision.** Only the project's (or member's) own `[dev-dependencies]` are read — a dependency's are its
own business. They resolve and lock with the rest of the graph. A reference through a dev-only alias is
allowed only in a suite; from a commons, context or adapter it is `bynk.package.dev_dependency_in_source`.
Slice 2a's reach-closure roots at suites only when the build checks them (`tests`), so a deployable build
compiles, and emits, no dev-dependency unit.

**Consequences.** A suite can target a dev-dependency's unit directly (`suite fx.sample`).
