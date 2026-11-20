---
level: patch
probes: emit_diagnostics, diagnostic_coverage, explained_codes
changelog: A `system` case's `is` test on a handler result whose type comes from a commons is checked again, so a wrong variant such as `r is Created(_)` on an `HttpOutcome[Note]` is `bynk.types.is_unknown_variant` instead of a failure in `tsc`; and a service with two `on call` handlers, where the second silently replaced the first, is now `bynk.service.duplicate_call_handler`
breaking: A service that declares two `on call` handlers no longer compiles (`bynk.service.duplicate_call_handler`). Only the last one ever ran, so delete the other, or move it into a service of its own.
---
