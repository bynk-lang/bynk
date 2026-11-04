---
level: patch
changelog: "A `let` bound to a literal `RequestHeaders.of([...])` and sent as extra request headers is checked for `Authorization` (`bynk.test.extra_headers_authorization`), and the below-`system` extra-headers error points at the offending call"
probes: emit_diagnostics, diagnostic_coverage
---
