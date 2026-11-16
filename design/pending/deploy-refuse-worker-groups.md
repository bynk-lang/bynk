---
level: patch
changelog: `bynk deploy`, `bynk revert` and `bynk dev` refuse a project with a `[deploy.workers]` group of more than one context, until grouping is deployable (#3562)
probes: diagnostic_coverage, emit_diagnostics, explained_codes
breaking: A `bynk.toml` with a `[deploy.workers.<key>]` group of more than one context is refused by `bynk deploy`, `bynk revert` and `bynk dev` with `bynk.deploy.worker_groups_unsupported`, because grouping is not yet deployable; remove the group, or keep it to a single context, until it is.
---

Temporary, and removed when `bynk deploy` (G4) and `bynk dev` (G6) learn groups.
`bynk build` emits a group as one Worker, but these commands still name each
Worker after its context, so they would deploy, revert or run the wrong thing.
A group of one context, and a project with no table, behave exactly as before.
