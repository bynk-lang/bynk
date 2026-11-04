---
level: patch
changelog: "A `Wire(...)` argument rejected on an event, queue or other test address is still type-checked, so an unknown name inside it is reported alongside `bynk.test.wire_http_only`"
probes: emit_diagnostics, diagnostic_coverage
---
