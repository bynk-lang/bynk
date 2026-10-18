---
level: patch
probes: emit_diagnostics, diagnostic_coverage
changelog: A `system`-tier case that runs a cron handler with `svc.schedule(…)` or delivers a queue message with `svc.message(…)` is now `bynk.test.schedule_needs_in_process` or `bynk.test.message_needs_in_process`, even when the target also exposes an `http` service, instead of passing check and emitting TypeScript that `tsc` rejects; run such a case `as integration`
---
