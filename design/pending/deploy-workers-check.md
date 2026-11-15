---
level: minor
changelog: `bynk check` and `bynk build` refuse a `[deploy.workers.<key>]` group that names an undeclared unit, a unit that is not a `context`, or a key that is an ungrouped context's Worker, in the editor too (#3562, G1)
probes: diagnostic_coverage, emit_diagnostics
---
