---
level: minor
changelog: A suite file may have its own `consumes`, and a suite file consumes what it names: naming a type that the suite's own `consumes` does not bring in (the target's clauses are not inherited) is `bynk.consumes.type_not_selected`, with a fix that edits the suite file (#3536)
probes: emit_diagnostics, diagnostic_coverage
---
