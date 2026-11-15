---
level: minor
changelog: `bynk build` refuses a `[deploy.workers.<key>]` group whose members declare different bindings under one name; identical declarations share one binding (#3562, G2a)
probes: diagnostic_coverage, emit_diagnostics, explained_codes
---
