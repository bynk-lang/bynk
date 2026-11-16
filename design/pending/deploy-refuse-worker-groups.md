---
level: patch
changelog: `bynk deploy`, `bynk revert` and `bynk dev` refuse a project with a `[deploy.workers]` group, until grouping is deployable (#3562)
probes: diagnostic_coverage, emit_diagnostics, explained_codes
---

Temporary, and removed when `bynk deploy` (G4) and `bynk dev` (G6) learn groups.
A group changes which Worker a context is in (its script name, directory, routes
and domains), but these commands still name each Worker after its context, so
they would deploy, revert or run the wrong thing. Any group is refused, a group
of one context included; a project with no table behaves exactly as before.
