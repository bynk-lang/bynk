---
level: patch
changelog: `bynk deploy`, `bynk revert` and `bynk dev` refuse a project with a `[deploy.workers]` group of more than one context, until grouping is deployable (#3562)
probes: diagnostic_coverage, emit_diagnostics, explained_codes
---

Temporary, and removed when `bynk deploy` (G4) and `bynk dev` (G6) learn groups.
`bynk build` emits a group as one Worker, but these commands still name each
Worker after its context, so they would deploy, revert or run the wrong thing.
A group of one context, and a project with no table, behave exactly as before.
