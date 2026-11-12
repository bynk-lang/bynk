---
level: minor
changelog: `bynk deploy` mints a `SignedCookie` actor's signing key when it is unset, keeps a held one, and refuses a weak supplied one; `bynk dev` uses a per-run key (#3386)
probes: emit_diagnostics, diagnostic_coverage, explained_codes, ts_writes
---
