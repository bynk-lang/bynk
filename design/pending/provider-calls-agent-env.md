---
level: patch
changelog: A context-local provider that calls one of its context's agents now reaches it — on `workers` the provider receives the Worker's `env` through its deps, where it used to fault with `ReferenceError: deps is not defined`
---
