---
level: patch
changelog: Internal restructuring: the Worker entry generator's call, admin, scheduled, queue and email families move into modules of their own; no output changes
---
refactor: split the Worker entry's non-HTTP families out of workers_entry.rs (2.4a)

Part of #3676. Part of #4089.

No output changes. `respond`, `wire`, `call`, `admin`, `scheduled` and `queue` move out of `bynk-emit/src/emitter/workers_entry.rs`, and `workers_email.rs` moves in as `workers_entry/email.rs`. The parent re-exports what outside code names. `ast_importers` gains three origin rows and reads the same.
