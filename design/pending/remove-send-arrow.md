---
level: minor
changelog: Remove the `~>` send, deprecated since v0.603.10 for `e.detach()`; a program that still uses it is refused with `bynk.parse.removed_send_arrow`, and the `bynk.send.*` codes are retired
probes: emit_diagnostics, ts_writes, diagnostic_coverage, explained_codes
breaking: `~>` is removed; run `bynk check --fix` on v0.603.x first, or rewrite `~> e` as `e.detach()` (written `do e.detach()` as a statement). A program that still contains `~>` fails with `bynk.parse.removed_send_arrow`.
---
