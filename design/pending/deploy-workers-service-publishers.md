---
level: minor
changelog: `bynk build` refuses a `[deploy.workers.<key>]` group whose members declare a service of the same name, and, for now, one with more than one publishing member (#3562, G2b)
probes: diagnostic_coverage, emit_diagnostics, explained_codes
---

Note for the G7 ADR: the `bynk.deploy.multiple_publishers` refusal is temporary and goes when G3 merges a group's events fan-out into one class.
