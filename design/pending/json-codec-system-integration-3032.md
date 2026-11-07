---
level: patch
changelog: "`Json.encode` / `Json.decode` / `Json.encodePlain` work in `system` and `integration` suites (#3032)"
probes: emit_diagnostics, ts_writes
---

A `system` or `integration` suite that called `Json.encode` or `Json.decode[T]` compiled to a module referencing `__serialise_*` / `__deserialise_*` helpers nothing defined, so `tsc` failed. The integration-module emitter now drains the codec roots its case bodies record and generates the closure itself, as the unit-suite emitter does (#917): helpers are built over the harness view's types and name each through the namespace it is imported under (the `uses` commons, the owners of reached types, the participants). It also raises the plain-`Option` switch when any source mentions `Json.encodePlain` (#2991), so plain mode works in these suites. Projects whose integration suites call no codec are byte-identical.
