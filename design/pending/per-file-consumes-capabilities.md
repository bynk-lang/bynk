---
level: minor
changelog: Capabilities and cross-context calls are per file too (warning stage): naming a flattened capability (`given Cap`, `Cap.op`, `provides`, a suite's `stub`) or calling another unit's service in a file whose own `consumes` does not bring it in is `bynk.consumes.capability_not_selected` or `bynk.consumes.unit_not_consumed`, with a fix that edits that file; the unit's dependencies are still the union of its files' `consumes` (#3536)
probes: emit_diagnostics, diagnostic_coverage, explained_codes
---
