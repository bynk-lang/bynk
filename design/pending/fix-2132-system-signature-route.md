---
level: patch
probes: emit_diagnostics, diagnostic_coverage
changelog: A `system`-tier case that calls a route whose actor is `Signature`-secured is now `bynk.test.signature_needs_in_process`, instead of passing check and sending an unsigned request the seam can only refuse; the `system` driver has no `Signature` signer yet, so call such a route from a `unit` or `integration` case
---
