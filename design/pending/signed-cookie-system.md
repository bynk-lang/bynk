---
level: minor
changelog: A `system` suite signs in to a `SignedCookie` route as `by User("ada")`, and `by Nobody`, `by Forged` and `by Expired` probe its seam; a case's `Sec-Fetch-Site` and `Origin` headers probe the CSRF check, and a `Sec-Fetch-Site: none` request is refused on a state-changing route whatever its `Origin` (#3386)
probes: emit_diagnostics, diagnostic_coverage, explained_codes, ts_writes
---
