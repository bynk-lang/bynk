---
level: minor
changelog: A route guarded by a `SignedCookie` actor verifies the session cookie, refuses a cross-site state-changing request, and hides the credential from the handler (#3386)
probes: emit_diagnostics, diagnostic_coverage, explained_codes
---
