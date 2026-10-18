---
level: patch
probes: emit_diagnostics, diagnostic_coverage
changelog: A `system`-tier case that calls a route whose actor is `Oidc`-secured is now `bynk.test.oidc_needs_in_process`, instead of passing check and sending a request with no token that the seam can only refuse; the `system` driver has no `Oidc` token issuer yet, so call such a route from a `unit` or `integration` case
---
